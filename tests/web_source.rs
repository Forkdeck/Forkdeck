//! Keep sample fixtures on the server side of the API boundary.
#[test]
fn browser_source_has_no_fixture_content_or_mode_switches() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    // Include both board titles and task/plan titles from the approved references.
    let titles = [
        "semantic-context#24 Home sync worker", "Web UI: read-only board (milestone 2)",
        "semantic-context#22 Home record API", "Forkdeck: unverified work never reads as done",
        "semantic-context#21 Persisted home records", "Fix four ways a task could start early",
        "Planners split multi-PR work into a task list", "Plans render a readable task list (first try)",
        "A plan can carry a validated task list", "Home records and sync", "Web UI and server mode",
        "Unverified work never reads as done", "Persisted home records", "Home record API",
        "Home sync worker", "Storage", "API", "Pick storage backend", "Docs for the home API",
    ];
    let mut dirs = vec![root.join("web/src")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() { dirs.push(path); continue; }
            let source = std::fs::read_to_string(&path).unwrap();
            assert!(!source.to_lowercase().split(|c: char| !c.is_alphanumeric()).any(|word| word == "demo"), "mode switch in {}", path.display());
            for title in titles { assert!(!source.contains(title), "sample title {title:?} in {}", path.display()); }
        }
    }
}
