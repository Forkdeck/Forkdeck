//! The task list a plan carries: parse, validate and order. Pure; no forge calls.
// Nothing outside the tests calls this yet; the callers arrive with the next tasks, which drop this allow.
#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const VERSION: u32 = 1;
pub const TAG: &str = "forkdeck-tasks";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Work,
    Group,
    Decision,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    pub kind: Kind,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub after: Vec<String>,
    #[serde(default)]
    pub checks: Vec<String>,
    #[serde(default)]
    pub number: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Graph {
    pub version: u32,
    pub tasks: Vec<Task>,
}

impl Graph {
    pub fn task(&self, id: &str) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// Direct children of a group; `None` is the top level.
    pub fn children(&self, id: Option<&str>) -> Vec<&Task> {
        self.tasks.iter().filter(|t| t.parent.as_deref() == id).collect()
    }

    /// The task's own `after` plus every ancestor group's, deduped, nearest first.
    pub fn effective_after(&self, id: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        let mut cur = self.task(id);
        while let Some(t) = cur {
            if !seen.insert(t.id.as_str()) {
                break;
            }
            for a in &t.after {
                if !out.contains(a) {
                    out.push(a.clone());
                }
            }
            cur = t.parent.as_deref().and_then(|p| self.task(p));
        }
        out
    }

    /// Indices with every parent and `after` target ahead of the task that needs it.
    pub fn creation_order(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for i in 0..self.tasks.len() {
            self.emit(i, &mut seen, &mut out);
        }
        out
    }

    fn emit(&self, i: usize, seen: &mut HashSet<usize>, out: &mut Vec<usize>) {
        if !seen.insert(i) {
            return;
        }
        let t = &self.tasks[i];
        let mut needs: Vec<String> = t.parent.iter().cloned().collect();
        needs.extend(self.effective_after(&t.id));
        for id in needs {
            if let Some(j) = self.tasks.iter().position(|x| x.id == id) {
                self.emit(j, seen, out);
            }
        }
        out.push(i);
    }

    fn descendants(&self, id: &str, out: &mut Vec<usize>) {
        for (i, t) in self.tasks.iter().enumerate() {
            if t.parent.as_deref() == Some(id) && !out.contains(&i) {
                out.push(i);
                self.descendants(&t.id, out);
            }
        }
    }

    /// What must finish before task `i` can start: each `after` target and, for a group, everything inside it.
    fn waits_on(&self, i: usize) -> Vec<usize> {
        let mut out = Vec::new();
        for id in self.effective_after(&self.tasks[i].id) {
            if let Some(j) = self.tasks.iter().position(|x| x.id == id) {
                if !out.contains(&j) {
                    out.push(j);
                }
                self.descendants(&id, &mut out);
            }
        }
        out
    }
}

/// Line span (start, end inclusive) of every fenced `forkdeck-tasks` block.
fn blocks(lines: &[&str]) -> Result<Vec<(usize, usize)>, String> {
    let opener = format!("```{TAG}");
    let mut out = Vec::new();
    let mut open: Option<usize> = None;
    for (n, l) in lines.iter().enumerate() {
        let l = l.trim_end();
        match open {
            None if l == opener => open = Some(n),
            Some(s) if l == "```" => {
                out.push((s, n));
                open = None;
            }
            _ => {}
        }
    }
    if open.is_some() {
        return Err(format!("The {TAG} block is not closed."));
    }
    Ok(out)
}

pub fn extract(text: &str) -> Result<Option<Graph>, String> {
    let lines: Vec<&str> = text.lines().collect();
    let found = blocks(&lines)?;
    match found.as_slice() {
        [] => Ok(None),
        [(s, e)] => serde_json::from_str(&lines[s + 1..*e].join("\n"))
            .map(Some)
            .map_err(|e| format!("The {TAG} block is not a valid task list: {e}")),
        _ => Err(format!("The plan has more than one {TAG} block.")),
    }
}

pub fn strip(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let Ok(found) = blocks(&lines) else { return text.to_string() };
    let mut out: Vec<&str> = Vec::new();
    let mut n = 0;
    while n < lines.len() {
        if let Some((_, e)) = found.iter().find(|(s, _)| *s == n) {
            n = e + 1;
            // Don't leave a double blank line where the block was.
            if out.last().is_some_and(|l| l.trim().is_empty()) && lines.get(n).is_some_and(|l| l.trim().is_empty()) {
                n += 1;
            }
            continue;
        }
        out.push(lines[n]);
        n += 1;
    }
    out.join("\n")
}

pub fn validate(g: &Graph) -> Result<(), String> {
    if g.version != VERSION {
        return Err(format!("Unsupported task list version {} (expected {VERSION}).", g.version));
    }
    if g.tasks.is_empty() {
        return Err("The task list is empty.".into());
    }
    let mut ids: HashSet<&str> = HashSet::new();
    for t in &g.tasks {
        if !ids.insert(t.id.as_str()) {
            return Err(format!("Task id {} appears twice.", t.id));
        }
    }
    if let Some(t) = g.tasks.iter().find(|t| t.title.trim().is_empty()) {
        return Err(format!("Task {} has no title.", t.id));
    }
    for t in &g.tasks {
        if let Some(a) = t.after.iter().find(|a| !ids.contains(a.as_str())) {
            return Err(format!("Task {} waits on unknown task {a}.", t.id));
        }
        if let Some(p) = t.parent.as_ref().filter(|p| !ids.contains(p.as_str())) {
            return Err(format!("Task {} is inside unknown task {p}.", t.id));
        }
    }
    if let Some(t) = g.tasks.iter().find(|t| t.after.contains(&t.id)) {
        return Err(format!("Task {} waits on itself.", t.id));
    }
    for t in &g.tasks {
        if let Some(p) = t.parent.as_ref().filter(|p| g.task(p).is_some_and(|x| x.kind != Kind::Group)) {
            return Err(format!("Task {} is inside {p}, which is not a group.", t.id));
        }
    }
    for t in g.tasks.iter().filter(|t| t.kind == Kind::Group) {
        if g.children(Some(&t.id)).is_empty() {
            return Err(format!("Group {} has no tasks.", t.id));
        }
    }
    for t in &g.tasks {
        let (mut cur, mut hops) = (t.parent.as_deref(), 0);
        while let Some(p) = cur {
            hops += 1;
            if p == t.id || hops > g.tasks.len() {
                return Err(format!("Task {} is inside itself (cycle of groups).", t.id));
            }
            cur = g.task(p).and_then(|x| x.parent.as_deref());
        }
    }
    // 0 = unvisited, 1 = on the current path, 2 = finished.
    fn visit(g: &Graph, i: usize, state: &mut [u8]) -> Result<(), String> {
        match state[i] {
            1 => return Err(format!("Task {} is part of a dependency cycle.", g.tasks[i].id)),
            2 => return Ok(()),
            _ => {}
        }
        state[i] = 1;
        for j in g.waits_on(i) {
            visit(g, j, state)?;
        }
        state[i] = 2;
        Ok(())
    }
    let mut state = vec![0u8; g.tasks.len()];
    for i in 0..g.tasks.len() {
        visit(g, i, &mut state)?;
    }
    Ok(())
}

pub const OPEN: &str = "<!-- forkdeck:tasks:start -->";
pub const CLOSE: &str = "<!-- forkdeck:tasks:end -->";
const EDITED: &str = "The task list on the issue was edited by hand; re-plan to change it.";

fn number_ref(t: &Task) -> String {
    t.number.map_or("#?".into(), |n| format!("#{n}"))
}

fn line_text(g: &Graph, t: &Task) -> String {
    let mut s = format!("{} {}", number_ref(t), t.title.replace('\n', " "));
    if !t.after.is_empty() {
        let refs: Vec<String> = t.after.iter().map(|a| g.task(a).map_or(format!("#{a}"), number_ref)).collect();
        s.push_str(&format!(" · after {}", refs.join(", ")));
    }
    s
}

fn list_lines(g: &Graph, parent: Option<&str>, depth: usize, status: &dyn Fn(&Task) -> String, out: &mut Vec<String>) {
    for t in g.children(parent) {
        let st = status(t);
        let done = st.starts_with("merged") || st == "done";
        let mut line = format!("{}- [{}] {}", "  ".repeat(depth), if done { 'x' } else { ' ' }, line_text(g, t));
        if !st.is_empty() {
            line.push_str(&format!(" — {st}"));
        }
        out.push(line);
        if t.kind == Kind::Group {
            list_lines(g, Some(&t.id), depth + 1, status, out);
        }
    }
}

/// The whole managed section, markers included. Statuses are display only and are never read back.
pub fn render(g: &Graph, status: &dyn Fn(&Task) -> String) -> String {
    let mut lines = Vec::new();
    list_lines(g, None, 0, status, &mut lines);
    let data = serde_json::to_string(g).unwrap_or_default();
    format!("{OPEN}\n## Tasks\n\n{}\n\n<details><summary>Plan data</summary>\n\n```json\n{data}\n```\n\n</details>\n{CLOSE}", lines.join("\n"))
}

/// Every visible line of a section around its plan data: the list and its headings before the data
/// fence, and the closing text after it. Blank lines carry no meaning.
fn visible(section: &str, from: usize, to: usize) -> Vec<&str> {
    section[..from].lines().chain(section[to..].lines()).filter(|l| !l.trim().is_empty()).collect()
}

/// Do two visible lines agree? Task lines may differ in checkbox and trailing status only.
fn same_line(actual: &str, expected: &str) -> bool {
    match (unboxed(actual), unboxed(expected)) {
        (Some((ai, ar)), Some((ei, er))) => ai == ei && (ar == er || ar.strip_prefix(er).is_some_and(|tail| tail.starts_with(" — "))),
        (None, None) => actual == expected,
        _ => false,
    }
}

/// Indent and the text after a valid `- [ ] ` or `- [x] ` checkbox; None for any other line.
fn unboxed(line: &str) -> Option<(usize, &str)> {
    let rest = line.trim_start();
    let text = rest.strip_prefix("- [ ] ").or_else(|| rest.strip_prefix("- [x] "))?;
    Some((line.len() - rest.len(), text))
}

/// The byte range of the plan data inside a section.
fn data_span(section: &str) -> Result<(usize, usize), String> {
    let fence = "```json\n";
    let from = section.find(fence).ok_or("The task list on the issue has no plan data; re-plan to restore it.")? + fence.len();
    let to = section[from..].find("\n```").ok_or("The task list on the issue has unreadable plan data; re-plan to restore it.")? + from;
    Ok((from, to))
}

/// The task list from a section, refused when anything visible no longer matches its data.
pub fn read(section: &str) -> Result<Graph, String> {
    let (from, to) = data_span(section)?;
    let g: Graph = serde_json::from_str(&section[from..to]).map_err(|e| format!("The task list's plan data is unreadable: {e}"))?;
    validate(&g)?;
    let rendered = render(&g, &|_| String::new());
    let (rf, rt) = data_span(&rendered)?;
    let (actual, expected) = (visible(section, from, to), visible(&rendered, rf, rt));
    let same = actual.len() == expected.len() && actual.iter().zip(&expected).all(|(a, e)| same_line(a, e));
    if same { Ok(g) } else { Err(EDITED.into()) }
}

pub fn task_marker(token: &str, id: &str, root: u64) -> String {
    format!("<!-- forkdeck:task {token} {id} root={root} -->")
}

pub fn parse_task_marker(body: &str) -> Option<(String, String, u64)> {
    static MARKER: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?s)<!-- forkdeck:task (\S+) (.*?) root=(\d+) -->").unwrap());
    let c = MARKER.captures(body)?;
    Some((c[1].to_string(), c[2].to_string(), c[3].parse().ok()?))
}

pub fn task_body(t: &Task, g: &Graph, token: &str, root: u64) -> String {
    let mut s = format!("{}\n\n{}\n\nPart of #{root}", task_marker(token, &t.id, root), t.body);
    let deps: Vec<String> = t.after.iter().filter_map(|a| g.task(a)?.number).map(|n| format!("#{n}")).collect();
    if !deps.is_empty() {
        s.push_str(&format!("\n\nDepends on {}", deps.join(", ")));
    }
    if !t.checks.is_empty() {
        s.push_str("\n\n**Acceptance checks**\n");
        s.push_str(&t.checks.iter().map(|c| format!("- {c}")).collect::<Vec<_>>().join("\n"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: &str = "Intro.\n\n```forkdeck-tasks\n{\"version\":1,\"tasks\":[\
 {\"id\":\"g0\",\"title\":\"Storage\",\"kind\":\"group\"},\
 {\"id\":\"a\",\"title\":\"Records\",\"kind\":\"work\",\"parent\":\"g0\"},\
 {\"id\":\"g1\",\"title\":\"API\",\"kind\":\"group\",\"after\":[\"g0\"]},\
 {\"id\":\"b\",\"title\":\"Endpoint\",\"kind\":\"work\",\"parent\":\"g1\"},\
 {\"id\":\"c\",\"title\":\"Sync\",\"kind\":\"work\",\"parent\":\"g1\",\"after\":[\"b\"]},\
 {\"id\":\"d\",\"title\":\"Pick backend\",\"kind\":\"decision\"}]}\n```\n\nPLAN: READY";

    fn numbered() -> Graph {
        let mut g = extract(PLAN).unwrap().unwrap();
        for (i, t) in g.tasks.iter_mut().enumerate() {
            t.number = Some(20 + i as u64);
        }
        g
    }

    #[test]
    fn the_list_reads_like_the_design_and_reads_back() {
        let g = numbered();
        let s = render(&g, &|t| if t.id == "a" { "merged in PR #71".into() } else { String::new() });
        assert!(s.starts_with(OPEN) && s.ends_with(CLOSE));
        assert!(s.contains("- [x] #21 Records — merged in PR #71"));
        assert!(s.contains("  - [ ] #24 Sync · after #23"), "nested under its group, own deps only:\n{s}");
        assert!(s.contains("- [ ] #22 API · after #20"));
        assert!(s.contains("<details><summary>Plan data</summary>"));
        assert_eq!(read(&s).unwrap(), g);
        // Statuses change on every pass; they are not an edit.
        assert_eq!(read(&render(&g, &|_| "PR #9 open".into())).unwrap(), g);
    }

    #[test]
    fn a_hand_edited_list_is_refused() {
        let s = render(&numbered(), &|_| String::new());
        assert!(read(&s.replace("#24 Sync", "#24 Sync faster")).unwrap_err().contains("edited"));
        assert!(read(&s.replace(" · after #23", "")).unwrap_err().contains("edited"));
        // A line the list would not have rendered is an edit, even one that merely looks like another task.
        assert!(read(&s.replace("- [ ] #24 Sync", "- [---#24 Sync")).unwrap_err().contains("edited"));
        assert!(read(&s.replace("- [ ] #24 Sync", "- [y] #24 Sync")).unwrap_err().contains("edited"));
        assert!(read(&s.replace("\n\n<details>", "\n+ [ ] #99 New prerequisite\n\n<details>")).unwrap_err().contains("edited"));
        assert!(read(&s.replace("</details>", "</details>\n- [ ] #99 New prerequisite")).unwrap_err().contains("edited"));
        assert!(read(&s.replace("\"version\":1", "\"version\":1,")).is_err(), "broken JSON");
    }

    #[test]
    fn task_issues_say_where_they_belong() {
        let g = numbered();
        let body = task_body(g.task("c").unwrap(), &g, "tok-3", 7);
        assert_eq!(parse_task_marker(&body), Some(("tok-3".into(), "c".into(), 7)));
        assert!(body.contains("Part of #7") && body.contains("Depends on #23"));
        assert_eq!(parse_task_marker("plain issue"), None);
        for id in ["sync records", "", " padded ", "a  b"] {
            assert_eq!(parse_task_marker(&task_marker("tok", id, 9)), Some(("tok".into(), id.into(), 9)), "id {id:?}");
        }
    }

    #[test]
    fn a_plan_carries_one_task_list() {
        let g = extract(PLAN).unwrap().unwrap();
        assert_eq!(g.tasks.len(), 6);
        validate(&g).unwrap();
        assert_eq!(extract("No list here.\nPLAN: READY").unwrap(), None);
        assert!(extract(&format!("{PLAN}\n{PLAN}")).is_err(), "two lists");
        assert!(extract("```forkdeck-tasks\nnot json\n```").is_err());
        assert!(!strip(PLAN).contains("forkdeck-tasks") && strip(PLAN).contains("Intro."));
    }

    #[test]
    fn group_dependencies_reach_everything_inside() {
        let g = extract(PLAN).unwrap().unwrap();
        assert_eq!(g.effective_after("c"), vec!["b".to_string(), "g0".to_string()]);
        assert_eq!(g.effective_after("a"), Vec::<String>::new());
        let order: Vec<&str> = g.creation_order().iter().map(|&i| g.tasks[i].id.as_str()).collect();
        let pos = |id| order.iter().position(|x| *x == id).unwrap();
        assert!(pos("g0") < pos("a") && pos("b") < pos("c") && pos("g1") < pos("b"));
    }

    #[test]
    fn the_first_problem_in_the_documented_order_is_the_one_reported() {
        let bad = |json: &str| validate(&serde_json::from_str::<Graph>(json).unwrap()).unwrap_err();
        // An unknown id outranks a self reference, even on the same task.
        assert!(bad(r#"{"version":1,"tasks":[{"id":"a","title":"A","kind":"work","after":["a","missing"]}]}"#).contains("missing"));
        // An empty title outranks a dependency problem on an earlier task.
        assert!(bad(r#"{"version":1,"tasks":[{"id":"a","title":"A","kind":"work","after":["zz"]},{"id":"b","title":" ","kind":"work"}]}"#).contains("no title"));
        // A duplicate id outranks an empty title.
        assert!(bad(r#"{"version":1,"tasks":[{"id":"a","title":"","kind":"work"},{"id":"a","title":"B","kind":"work"}]}"#).contains("twice"));
        // An unknown id on a later task outranks a self reference on an earlier one.
        assert!(bad(r#"{"version":1,"tasks":[{"id":"a","title":"A","kind":"work","after":["a"]},{"id":"b","title":"B","kind":"work","after":["zz"]}]}"#).contains("zz"));
        // A non-group parent outranks a group with no tasks.
        assert!(bad(r#"{"version":1,"tasks":[{"id":"g","title":"G","kind":"group"},{"id":"a","title":"A","kind":"work","parent":"b"},{"id":"b","title":"B","kind":"work"}]}"#).contains("not a group"));
    }

    #[test]
    fn broken_lists_are_refused_with_a_reason() {
        let bad = |json: &str| validate(&serde_json::from_str::<Graph>(json).unwrap()).unwrap_err();
        assert!(bad(r#"{"version":1,"tasks":[{"id":"a","title":"A","kind":"work","after":["a"]}]}"#).contains("itself"));
        assert!(bad(r#"{"version":1,"tasks":[{"id":"a","title":"A","kind":"work","after":["zz"]}]}"#).contains("zz"));
        assert!(bad(r#"{"version":1,"tasks":[{"id":"a","title":"A","kind":"work","after":["b"]},{"id":"b","title":"B","kind":"work","after":["a"]}]}"#).contains("cycle"));
        // A task waiting on the group it is inside can never start.
        assert!(bad(r#"{"version":1,"tasks":[{"id":"g","title":"G","kind":"group"},{"id":"a","title":"A","kind":"work","parent":"g","after":["g"]}]}"#).contains("cycle"));
        assert!(bad(r#"{"version":1,"tasks":[{"id":"g","title":"G","kind":"group"}]}"#).contains("no tasks"));
        assert!(bad(r#"{"version":1,"tasks":[{"id":"a","title":"A","kind":"work"},{"id":"a","title":"B","kind":"work"}]}"#).contains("twice"));
        assert!(bad(r#"{"version":1,"tasks":[{"id":"a","title":"A","kind":"work","parent":"b"},{"id":"b","title":"B","kind":"work"}]}"#).contains("group"));
        assert!(bad(r#"{"version":2,"tasks":[{"id":"a","title":"A","kind":"work"}]}"#).contains("version"));
        assert!(bad(r#"{"version":1,"tasks":[]}"#).contains("empty"));
    }
}
