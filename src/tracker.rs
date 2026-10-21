//! Forge boundary for issue plans. Keys stay opaque outside this adapter.
use crate::{config::RepoCfg, issues, state::Provider, team};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const OPEN: &str = "<!-- forkdeck:plan:start -->";
pub const CLOSE: &str = "<!-- forkdeck:plan:end -->";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct IssueRef {
    pub provider: Provider,
    pub repo: String,
    pub key: String,
    pub url: String,
}

#[derive(Clone, Debug)]
pub struct RemoteIssue {
    pub reference: IssueRef,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub open: bool,
}

pub fn section(body: &str) -> Result<Option<String>, String> { section_between(body, OPEN, CLOSE) }

pub fn section_between(body: &str, open: &str, close: &str) -> Result<Option<String>, String> {
    let starts: Vec<_> = body.match_indices(open).collect();
    let ends: Vec<_> = body.match_indices(close).collect();
    match (starts.as_slice(), ends.as_slice()) {
        ([], []) => Ok(None),
        ([(a, _)], [(b, _)]) if a < b => Ok(Some(body[*a..*b + close.len()].into())),
        _ => Err("The issue has ambiguous plan markers; repair them on the forge before publishing.".into()),
    }
}

pub fn replace(body: &str, expected: &Option<String>, replacement: &str) -> Result<String, String> {
    replace_between(body, OPEN, CLOSE, expected, replacement)
}

pub fn replace_between(body: &str, open: &str, close: &str, expected: &Option<String>, replacement: &str) -> Result<String, String> {
    let current = section_between(body, open, close)?;
    if &current != expected { return Err("The issue plan changed remotely. Refine against the latest issue before accepting again.".into()); }
    Ok(match current {
        Some(old) => body.replacen(&old, replacement, 1),
        None if body.is_empty() => replacement.into(),
        None => format!("{body}\n\n{replacement}"),
    })
}

pub fn managed(plan: &str, token: &str, revision: u64) -> String {
    format!("{OPEN}\n<!-- forkdeck:revision {token} -->\n## Implementation plan · revision {revision}\n\n{plan}\n{CLOSE}")
}

/// The forge refused the request outright (a 4xx answer), so it did nothing. A timeout (408), a
/// rate limit (429), a server error or no answer at all may still have taken effect.
pub fn refused(err: &str) -> bool {
    static STATUS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"\(HTTP (\d{3})\)").unwrap());
    STATUS.captures(err).and_then(|c| c[1].parse::<u16>().ok()).is_some_and(|s| (400..500).contains(&s) && s != 408 && s != 429)
}

/// Does the issue carry `label`? GitHub label names ignore case; GitLab's do not.
pub fn has_label(r: &RepoCfg, labels: &[String], label: &str) -> bool {
    labels.iter().any(|l| if r.provider == Provider::Github { l.eq_ignore_ascii_case(label) } else { l == label })
}

fn tool(r: &RepoCfg) -> &'static str { if r.provider == Provider::Github { "gh" } else { "glab" } }
fn root(r: &RepoCfg) -> String {
    if r.provider == Provider::Github { format!("repos/{}/issues", r.repo) }
    else { format!("projects/{}/issues", encode(&r.repo)) }
}
fn encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}
pub fn number(key: &str) -> Result<u64, String> {
    key.parse::<u64>().ok().filter(|n| *n > 0).ok_or_else(|| "Expected a positive issue number.".into())
}

/// URLs must identify the configured repository. The API response is the canonical URL.
pub fn key(r: &RepoCfg, input: &str) -> Result<String, String> {
    let s = input.trim();
    if !s.contains("://") { return number(s.trim_start_matches('#')).map(|n| n.to_string()); }
    let url = reqwest::Url::parse(s).map_err(|_| "Invalid issue URL")?;
    if !matches!(url.scheme(), "http" | "https") || url.query().is_some() || url.fragment().is_some() {
        return Err("Use an issue URL without a query or fragment.".into());
    }
    // Checked before anything is attached: a link to the same path on another host would otherwise
    // be saved and then refused on every turn, holding the issue up.
    if r.host.contains('.') && !url.host_str().is_some_and(|h| h.eq_ignore_ascii_case(&r.host)) {
        return Err(format!("Issue URL is not on {}, where the selected repository's clone points.", r.host));
    }
    let prefix = if r.provider == Provider::Github { format!("/{}/issues/", r.repo) } else { format!("/{}/-/issues/", r.repo) };
    let n = url.path().strip_prefix(&prefix).ok_or("Issue URL does not match the selected repository.")?;
    number(n).map(|n| n.to_string())
}

async fn api(r: &RepoCfg, method: &str, path: &str, data: Option<Value>) -> Result<String, String> {
    let dir = if r.path.is_dir() { Some(r.path.as_path()) } else { None };
    let host = r.host_args();
    let mut args: Vec<&str> = vec!["api"];
    args.extend(host.iter().map(String::as_str));
    if let Some(data) = data {
        args.extend(["-X", method, path, "--input", "-", "-H", "Content-Type: application/json"]);
        issues::sh_in(tool(r), &args, dir, &data.to_string()).await
    } else {
        args.push(path);
        issues::sh(tool(r), &args, dir).await
    }
}

fn parse(r: &RepoCfg, v: &Value) -> Result<RemoteIssue, String> {
    if v.get("pull_request").is_some() { return Err("Select an issue, not a pull request.".into()); }
    let github = r.provider == Provider::Github;
    let key = v[if github { "number" } else { "iid" }].as_u64().filter(|n| *n > 0).ok_or("Forge response has no issue number")?.to_string();
    let url = v[if github { "html_url" } else { "web_url" }].as_str().ok_or("Forge response has no issue URL")?.into();
    Ok(RemoteIssue {
        reference: IssueRef { provider: r.provider, repo: r.repo.clone(), key, url },
        title: v["title"].as_str().unwrap_or("").into(),
        body: v[if github { "body" } else { "description" }].as_str().unwrap_or("").into(),
        labels: v["labels"].as_array().into_iter().flatten().filter_map(|l| l.as_str().or_else(|| l["name"].as_str())).map(str::to_string).collect(),
        open: matches!(v["state"].as_str(), Some("open" | "opened")),
    })
}

pub async fn read(r: &RepoCfg, key: &str) -> Result<RemoteIssue, String> {
    let out = api(r, "GET", &format!("{}/{}", root(r), number(key)?), None).await?;
    parse(r, &serde_json::from_str(&out).map_err(|e| format!("Invalid issue response: {e}"))?)
}

pub async fn read_ref(r: &RepoCfg, reference: &IssueRef) -> Result<RemoteIssue, String> {
    if reference.repo != r.repo || reference.provider != r.provider { return Err("Issue reference does not match the configured repository.".into()); }
    let remote = read(r, &reference.key).await?;
    if !reference.url.is_empty() {
        let requested = reqwest::Url::parse(&reference.url).map_err(|_| "Invalid saved issue URL")?;
        let actual = reqwest::Url::parse(&remote.reference.url).map_err(|_| "Invalid forge issue URL")?;
        if requested.host_str() != actual.host_str() || requested.port_or_known_default() != actual.port_or_known_default() || requested.path() != actual.path() {
            return Err("The issue URL does not match the configured forge CLI. Configure that repository's CLI host first.".into());
        }
    }
    Ok(remote)
}

/// The forge answers and the CLI is logged in, right before a write whose outcome could be uncertain.
pub async fn reachable(r: &RepoCfg) -> Result<(), String> {
    api(r, "GET", "user", None).await.map(|_| ()).map_err(|e| format!("The forge cannot be reached, so nothing was sent: {e}"))
}

pub async fn create(r: &RepoCfg, title: &str, body: &str) -> Result<RemoteIssue, String> {
    let data = if r.provider == Provider::Github { json!({"title":title,"body":body}) } else { json!({"title":title,"description":body}) };
    let out = api(r, "POST", &root(r), Some(data)).await?;
    parse(r, &serde_json::from_str(&out).map_err(|e| format!("Invalid created issue response: {e}"))?)
}

/// Find an uncertain create by its durable marker, including closed issues and all pages.
pub async fn find_created(r: &RepoCfg, marker: &str) -> Result<Option<RemoteIssue>, String> {
    let dir = if r.path.is_dir() { Some(r.path.as_path()) } else { None };
    let (host, path) = (r.host_args(), format!("{}?state=all&per_page=100", root(r)));
    let mut args: Vec<&str> = vec!["api"];
    args.extend(host.iter().map(String::as_str));
    args.extend(["--paginate", &path]);
    let out = issues::sh(tool(r), &args, dir).await?;
    let mut found = None;
    for page in serde_json::Deserializer::from_str(&out).into_iter::<Value>() {
        let page = page.map_err(|e| e.to_string())?;
        let entries = page.as_array().ok_or("Invalid paginated issue response")?;
        for entry in entries {
            if entry.get("pull_request").is_some() { continue; }
            let item = parse(r, entry)?;
            if item.body.contains(marker) {
                if found.is_some() { return Err("Multiple issues have this publication marker; link the intended issue explicitly.".into()); }
                found = Some(item);
            }
        }
    }
    Ok(found)
}

pub async fn update(r: &RepoCfg, key: &str, body: &str) -> Result<(), String> {
    let (method, data) = if r.provider == Provider::Github { ("PATCH", json!({"body":body})) } else { ("PUT", json!({"description":body})) };
    api(r, method, &format!("{}/{}", root(r), number(key)?), Some(data)).await.map(|_| ())
}

pub async fn history(r: &RepoCfg, key: &str) -> Result<Vec<team::Comment>, String> {
    team::comments(r, &team::Target::Issue(number(key)?)).await
}
pub async fn comment(r: &RepoCfg, key: &str, body: &str) -> Result<(), String> {
    let path = team::list_path(r, &team::Target::Issue(number(key)?)).ok_or("Comments unsupported")?;
    api(r, "POST", &path, Some(json!({"body":body}))).await.map(|_| ())
}
pub async fn readiness(r: &RepoCfg, key: &str, ready: bool) -> Result<(), String> {
    // Label-only edits preserve unrelated labels on both forges.
    let path = format!("{}/{}", root(r), number(key)?);
    if r.provider == Provider::Github {
        if ready { api(r, "POST", &format!("{path}/labels"), Some(json!({"labels":[r.label]}))).await?; }
        else { api(r, "DELETE", &format!("{path}/labels/{}", encode(&r.label)), Some(json!({}))).await?; }
    } else {
        let data = if ready { json!({"add_labels":r.label}) } else { json!({"remove_labels":r.label}) };
        api(r, "PUT", &path, Some(data)).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_a_definite_refusal_counts_as_nothing_written() {
        assert!(refused("gh api: Resource not accessible by integration (HTTP 403)"));
        assert!(refused("glab api: glab: 422 Unprocessable Entity (HTTP 422)"));
        assert!(!refused("gh api: Server Error (HTTP 502)"));
        assert!(!refused("gh api: API rate limit exceeded (HTTP 429)"));
        assert!(!refused("gh api: timed out"));
    }

    #[test]
    fn an_issue_url_on_another_host_is_refused_before_anything_is_saved() {
        let mut cfg: crate::config::Config = toml::from_str("[[repos]]\nprovider='gitlab'\nrepo='a/b'\npath='/tmp'\n").unwrap();
        cfg.repos[0].host = "gitlab.one.example".into();
        let r = &cfg.repos[0];
        assert_eq!(key(r, "https://gitlab.one.example/a/b/-/issues/7").unwrap(), "7");
        assert!(key(r, "https://gitlab.two.example/a/b/-/issues/7").unwrap_err().contains("gitlab.one.example"));
        assert_eq!(key(r, "#7").unwrap(), "7", "a number has no host to check");
    }

    #[test]
    fn github_labels_match_whatever_their_case() {
        for (provider, matches) in [("github", true), ("gitlab", false)] {
            let cfg: crate::config::Config = toml::from_str(&format!("[[repos]]\nprovider='{provider}'\nrepo='a/b'\npath='/tmp'\n")).unwrap();
            assert_eq!(has_label(&cfg.repos[0], &["Agent:Working".into()], "agent:working"), matches, "{provider}");
            assert!(has_label(&cfg.repos[0], &["agent:working".into()], "agent:working"));
        }
    }

    #[test]
    fn preserve_human_text_and_detect_conflicts() {
        let old = managed("old", "a", 1);
        let body = format!("Human intro\n{old}\nHuman tail");
        let new = managed("new", "b", 2);
        assert_eq!(replace(&body, &Some(old.clone()), &new).unwrap(), format!("Human intro\n{new}\nHuman tail"));
        assert!(replace(&body, &None, &new).is_err());
        assert!(section(&format!("{old}\n{old}")).is_err());
        assert!(section(CLOSE).is_err());
    }

    #[test]
    fn links_and_payloads_are_provider_specific() {
        for provider in ["github", "gitlab"] {
            let cfg: crate::config::Config = toml::from_str(&format!("[[repos]]\nprovider='{provider}'\nrepo='a/b'\npath='/tmp'\n")).unwrap();
            let r = &cfg.repos[0];
            assert_eq!(key(r, "#42").unwrap(), "42");
            assert!(key(r, "0").is_err());
            assert!(key(r, "-3").is_err());
            assert!(key(r, "https://example.test/wrong/repo/issues/42").is_err());
            let path = if provider == "github" { "a/b/issues" } else { "a/b/-/issues" };
            assert_eq!(key(r, &format!("https://example.test/{path}/42")).unwrap(), "42");
            let github = provider == "github";
            let value = if github { json!({"number":42,"html_url":"https://example.test/a/b/issues/42","body":"Human body","labels":[{"name":"custom"}],"state":"open"}) }
                else { json!({"iid":42,"web_url":"https://example.test/a/b/-/issues/42","description":"Human body","labels":["custom"],"state":"opened"}) };
            let remote = parse(r, &value).unwrap();
            assert!(remote.open);
            assert_eq!(remote.body, "Human body");
            assert_eq!(remote.labels, ["custom"]);
            assert!(parse(r, &json!({"number":42,"pull_request":{}})).is_err());
        }
    }
}
