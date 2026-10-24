//! Repository discovery only. Provider tokens never leave the callback's stack.
use super::{auth::{cookie, Principal}, http::App};
use crate::{config::ForgeAppCfg, state::Provider};
use axum::{extract::{Path, Query, State}, http::{header, HeaderMap, StatusCode}, response::{IntoResponse, Redirect, Response}, Json};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::VecDeque, time::Duration};

pub(super) const COOKIE: &str = "forkdeck_forge";
fn now() -> i64 { chrono::Utc::now().timestamp() }
fn random() -> String {
    let mut bytes = [0; 32];
    getrandom::getrandom(&mut bytes).expect("OS randomness");
    URL_SAFE_NO_PAD.encode(bytes)
}
fn hash(value: &str) -> String { URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes())) }

struct Pending { state: String, binding: String, provider: Provider, verifier: String, expires: i64 }
struct Picker { id: String, binding: String, provider: Provider, host: String, repos: Vec<Repository>, expires: i64 }
#[derive(Clone, Serialize)]
struct Repository { repo: String, private: bool, default_branch: String, description: String }
#[derive(Default)]
pub(super) struct Forges { pending: VecDeque<Pending>, pickers: VecDeque<Picker> }
impl Forges {
    fn prune(&mut self) {
        self.pending.retain(|p| p.expires > now());
        self.pickers.retain(|p| p.expires > now());
    }
}

pub(super) struct PickedRepo { pub provider: Provider, pub host: String, pub repo: String, pub default_branch: String }
pub(super) fn picked(app: &App, picker: &str, cookie: &str, repo: &str) -> Option<PickedRepo> {
    let mut store = app.forges.lock().unwrap();
    store.prune();
    let p = store.pickers.iter().find(|p| p.id == picker && p.binding == hash(cookie))?;
    let r = p.repos.iter().find(|r| r.repo == repo)?;
    Some(PickedRepo { provider: p.provider, host: p.host.clone(), repo: r.repo.clone(), default_branch: r.default_branch.clone() })
}
fn provider(id: &str) -> Option<Provider> { match id { "github" => Some(Provider::Github), "gitlab" => Some(Provider::Gitlab), _ => None } }
fn id(provider: Provider) -> &'static str { match provider { Provider::Github => "github", Provider::Gitlab => "gitlab" } }
pub(super) fn config(app: &App, provider: Provider) -> Option<&ForgeAppCfg> {
    match provider { Provider::Github => app.hub.cfg.web.forges.github.as_ref(), Provider::Gitlab => app.hub.cfg.web.forges.gitlab.as_ref() }
}
pub(super) fn can_add(principal: Option<&Principal>) -> bool { super::auth::can_write(principal) }
fn binding(headers: &HeaderMap) -> String { cookie(headers, COOKIE).unwrap_or_default() }
fn with_cookie(mut response: Response, value: &str, path: &str, seconds: u16, cfg: &ForgeAppCfg) -> Response {
    let secure = if cfg.redirect_url.starts_with("https:") { "; Secure" } else { "" };
    let cookie = format!("{COOKIE}={value}; Path={path}; HttpOnly; SameSite=Lax; Max-Age={seconds}{secure}");
    response.headers_mut().append(header::SET_COOKIE,cookie.parse().unwrap());
    response.headers_mut().insert(header::CACHE_CONTROL,"no-store".parse().unwrap());
    response
}
fn endpoint(cfg: &ForgeAppCfg, provider: Provider, step: &str) -> String {
    let root = if cfg.base_url.is_empty() { format!("https://{}",cfg.host) } else { cfg.base_url.trim_end_matches('/').into() };
    match (provider,step) {
        (Provider::Github,"authorize") => format!("{root}/login/oauth/authorize"),
        (Provider::Github,"exchange") => format!("{root}/login/oauth/access_token"),
        (Provider::Github,_) if !cfg.base_url.is_empty() => format!("{root}/user/repos"),
        (Provider::Github,_) if cfg.host.eq_ignore_ascii_case("github.com") => "https://api.github.com/user/repos".into(),
        (Provider::Github,_) => format!("{root}/api/v3/user/repos"),
        (Provider::Gitlab,"authorize") => format!("{root}/oauth/authorize"),
        (Provider::Gitlab,"exchange") => format!("{root}/oauth/token"),
        (Provider::Gitlab,_) => format!("{root}/api/v4/projects"),
    }
}

pub(super) async fn connect(State(app): State<App>, Path(name): Path<String>, principal: Option<axum::Extension<Principal>>) -> Response {
    let Some(provider) = provider(&name) else { return StatusCode::NOT_FOUND.into_response() };
    let Some(cfg) = config(&app,provider) else { return StatusCode::NOT_FOUND.into_response() };
    if !can_add(principal.as_ref().map(|p| &p.0)) { return StatusCode::FORBIDDEN.into_response(); }
    let state = random();
    let cookie = random();
    let verifier = random();
    let mut url = reqwest::Url::parse(&endpoint(cfg,provider,"authorize")).unwrap();
    let scope = match provider { Provider::Github if cfg.private => "repo", Provider::Github => "", Provider::Gitlab => "read_api" };
    url.query_pairs_mut().append_pair("client_id",&cfg.client_id).append_pair("redirect_uri",&cfg.redirect_url)
        .append_pair("response_type","code").append_pair("scope",scope).append_pair("state",&state)
        .append_pair("code_challenge",&hash(&verifier)).append_pair("code_challenge_method","S256");
    let mut store = app.forges.lock().unwrap();
    store.prune();
    while store.pending.len() >= 64 { store.pending.pop_front(); }
    store.pending.push_back(Pending { state, binding:hash(&cookie), provider, verifier, expires:now()+300 });
    with_cookie(Redirect::to(url.as_str()).into_response(),&cookie,"/forges",300,cfg)
}

#[derive(Deserialize)]
pub(super) struct Callback { code: Option<String>, state: Option<String> }
fn failed(step: &str, status: u16) -> Response {
    (StatusCode::BAD_GATEWAY,format!("Forge {step} failed (HTTP {status})")).into_response()
}

pub(super) async fn callback(State(app): State<App>, Path(name): Path<String>, Query(query): Query<Callback>, headers: HeaderMap, principal: Option<axum::Extension<Principal>>) -> Response {
    let Some(provider) = provider(&name) else { return StatusCode::NOT_FOUND.into_response() };
    let Some(cfg) = config(&app,provider) else { return StatusCode::NOT_FOUND.into_response() };
    if !can_add(principal.as_ref().map(|p| &p.0)) { return StatusCode::FORBIDDEN.into_response(); }
    let cookie = binding(&headers);
    let flow = {
        let mut store = app.forges.lock().unwrap();
        store.prune();
        let Some(index) = store.pending.iter().position(|p| Some(&p.state) == query.state.as_ref() && p.provider == provider && p.binding == hash(&cookie)) else { return StatusCode::BAD_REQUEST.into_response() };
        store.pending.remove(index).unwrap()
    };
    let Some(code) = query.code.filter(|c| !c.is_empty()) else { return StatusCode::BAD_REQUEST.into_response() };
    let Ok(secret) = cfg.secret() else { return failed("exchange",503) };
    let Ok(http) = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(15)).build() else { return failed("exchange",503) };
    let exchange = http.post(endpoint(cfg,provider,"exchange")).header(header::ACCEPT,"application/json")
        .form(&[("grant_type","authorization_code"),("client_id",&cfg.client_id),("client_secret",&secret),("redirect_uri",&cfg.redirect_url),("code",&code),("code_verifier",&flow.verifier)]).send().await;
    let response = match exchange { Ok(r) => r, Err(_) => return failed("exchange",502) };
    let status = response.status().as_u16();
    if !response.status().is_success() { return failed("exchange",status); }
    #[derive(Deserialize)]
    struct Access { access_token: String }
    let token = match response.json::<Access>().await { Ok(t) if !t.access_token.is_empty() => t.access_token, _ => return failed("exchange",status) };
    let mut repos = Vec::new();
    for page in 1..=10 {
        let mut url = reqwest::Url::parse(&endpoint(cfg,provider,"listing")).unwrap();
        url.query_pairs_mut().append_pair("per_page","100").append_pair("page",&page.to_string());
        match provider {
            Provider::Github => { url.query_pairs_mut().append_pair("affiliation","owner,collaborator,organization_member"); }
            Provider::Gitlab => { url.query_pairs_mut().append_pair("membership","true").append_pair("simple","true"); }
        }
        let response = match http.get(url).bearer_auth(&token).header(header::USER_AGENT,"Forkdeck").header(header::ACCEPT,"application/json").send().await {
            Ok(r) => r, Err(_) => return failed("listing",502),
        };
        let status = response.status().as_u16();
        if !response.status().is_success() { return failed("listing",status); }
        // Only use pagination as a signal. Never send a bearer token to a provider-supplied URL.
        let more = response.headers().get("link").and_then(|h|h.to_str().ok()).is_some_and(|h| h.split(',').any(|part| part.contains("rel=\"next\"")))
            || response.headers().get("x-next-page").and_then(|h|h.to_str().ok()).is_some_and(|s| !s.is_empty());
        let values = match response.json::<Vec<serde_json::Value>>().await { Ok(v) => v, Err(_) => return failed("listing",status) };
        for value in values.into_iter().take(100) {
            let name = value[if provider == Provider::Github { "full_name" } else { "path_with_namespace" }].as_str().unwrap_or_default();
            if !valid_repo(name) || repos.iter().any(|r: &Repository| r.repo == name) { continue; }
            let private = if provider == Provider::Github { value["private"].as_bool().unwrap_or(true) } else { value["visibility"].as_str() != Some("public") };
            if provider == Provider::Github && private && !cfg.private { continue; }
            let branch = value["default_branch"].as_str().filter(|s| !s.is_empty()).unwrap_or("main");
            if name.contains(&token) || branch.contains(&token) { continue; }
            let description = value["description"].as_str().filter(|s| !s.contains(&token)).unwrap_or_default().to_owned();
            repos.push(Repository { repo:name.into(), private, default_branch:branch.into(), description });
        }
        if !more { break; }
    }
    drop(token);
    let id = random();
    let mut store = app.forges.lock().unwrap();
    store.prune();
    while store.pickers.len() >= 64 { store.pickers.pop_front(); }
    store.pickers.push_back(Picker { id:id.clone(), binding:hash(&cookie), provider, host:cfg.host.clone(), repos, expires:now()+600 });
    // The flow cookie stays scoped to /forges; the same binding must also reach browser writes.
    with_cookie(Redirect::to(&format!("/repos/add/{id}")).into_response(),&cookie,"/api/v1",600,cfg)
}

pub(super) fn valid_repo(repo: &str) -> bool {
    repo.len() <= 512 && repo.split('/').count() >= 2 && repo.split('/').all(|part| !part.is_empty() && part != "." && part != ".." && !part.starts_with('-') && part.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)))
}
pub(super) fn configured(app: &App, provider: Provider, host: &str, repo: &str) -> bool {
    app.hub.cfg.repos.iter().any(|r| r.provider == provider && r.repo == repo && repo_host(r).eq_ignore_ascii_case(host))
}
pub(super) fn repo_host(repo: &crate::config::RepoCfg) -> &str {
    if repo.host.is_empty() { match repo.provider { Provider::Github => "github.com", Provider::Gitlab => "gitlab.com" } } else { &repo.host }
}
pub(super) async fn providers(State(app): State<App>, principal: Option<axum::Extension<Principal>>) -> Response {
    let providers: Vec<_> = [Provider::Github,Provider::Gitlab].into_iter().filter_map(|p| config(&app,p).map(|c| json!({"id":id(p),"host":c.host}))).collect();
    if providers.is_empty() { return StatusCode::NOT_FOUND.into_response(); }
    Json(json!({"providers":providers,"can_add":can_add(principal.as_ref().map(|p| &p.0))})).into_response()
}
pub(super) async fn picker(State(app): State<App>, Path(picker_id): Path<String>, headers: HeaderMap) -> Response {
    let mut store = app.forges.lock().unwrap();
    store.prune();
    let Some(p) = store.pickers.iter().find(|p| p.id == picker_id && p.binding == hash(&binding(&headers))) else { return StatusCode::NOT_FOUND.into_response() };
    let repos: Vec<_> = p.repos.iter().map(|r| {
        let mut value = serde_json::to_value(r).unwrap();
        value["configured"] = json!(configured(&app,p.provider,&p.host,&r.repo));
        value
    }).collect();
    Json(json!({"provider":id(p.provider),"host":p.host,"expires_unix":p.expires,"repos":repos})).into_response()
}

#[cfg(test)]
pub(super) fn test_picker(app: &App, id: &str, cookie: &str, provider: Provider, host: &str, repos: &[&str]) {
    app.forges.lock().unwrap().pickers.push_back(Picker { id:id.into(),binding:hash(cookie),provider,host:host.into(),
        repos:repos.iter().map(|repo|Repository { repo:(*repo).into(),private:false,default_branch:"main".into(),description:String::new() }).collect(),expires:now()+600 });
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::http::App;
    use axum::{extract::{State, Path, Query}, http::{header, HeaderMap, StatusCode}, response::{Redirect, Response, IntoResponse}};
    use std::{collections::HashMap, sync::Mutex};
    use axum::{extract::Form, routing::{get, post}, Router, Json};
    use serde_json::{json, Value};
    use std::sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}};

    const TOKEN: &str = "secret-provider-token-must-never-escape";
    struct Fake {
        calls: AtomicUsize,
        fail: bool,
        echo_metadata: AtomicBool,
        codes: Mutex<HashMap<String, String>>,
    }
    async fn authorize(State(fake): State<Arc<Fake>>, Query(q): Query<HashMap<String,String>>) -> Redirect {
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["response_type"], "code");
        let code = random();
        fake.codes.lock().unwrap().insert(code.clone(), q["code_challenge"].clone());
        let mut url = reqwest::Url::parse(&q["redirect_uri"]).unwrap();
        url.query_pairs_mut().append_pair("code", &code).append_pair("state", &q["state"]);
        Redirect::to(url.as_str())
    }
    async fn token(State(fake): State<Arc<Fake>>, Form(q): Form<HashMap<String,String>>) -> Json<Value> {
        fake.calls.fetch_add(1, Ordering::SeqCst);
        let challenge = fake.codes.lock().unwrap().remove(&q["code"]).unwrap();
        assert_eq!(hash(&q["code_verifier"]), challenge);
        assert_eq!(q["client_secret"], "fixture-secret");
        assert_eq!(q["grant_type"], "authorization_code");
        Json(json!({"access_token":TOKEN,"token_type":"Bearer"}))
    }
    async fn listing(State(fake): State<Arc<Fake>>, Path(provider): Path<String>, Query(q): Query<HashMap<String,String>>, headers: HeaderMap) -> Response {
        assert_eq!(headers[header::AUTHORIZATION], format!("Bearer {TOKEN}"));
        assert_eq!(q["per_page"], "100");
        if fake.fail { return (StatusCode::INTERNAL_SERVER_ERROR, TOKEN).into_response(); }
        let page = q.get("page").map(String::as_str).unwrap_or("1");
        let description = if fake.echo_metadata.load(Ordering::SeqCst) { TOKEN } else { "Real metadata" };
        let repo = if page == "1" { "owner/already" } else { "owner/new" };
        let mut response = if provider == "user" {
            assert_eq!(q["affiliation"], "owner,collaborator,organization_member");
            Json(json!([{"full_name":repo,"private":true,"default_branch":"main","description":description}])).into_response()
        } else {
            assert_eq!(q["membership"], "true");
            assert_eq!(q["simple"], "true");
            Json(json!([{"path_with_namespace":repo,"visibility":"private","default_branch":"main","description":description}])).into_response()
        };
        if page == "1" {
            response.headers_mut().insert("link", "</user/repos?page=2>; rel=\"next\"".parse().unwrap());
            response.headers_mut().insert("x-next-page", "2".parse().unwrap());
        }
        response
    }
    struct Fixture {
        app: App,
        fake: Arc<Fake>,
        client: reqwest::Client,
        base: String,
        tasks: Vec<tokio::task::JoinHandle<()>>,
    }
    impl Drop for Fixture { fn drop(&mut self) { for task in &self.tasks { task.abort(); } } }
    impl Fixture {
        async fn new(fail: bool, enabled: bool, viewer: bool) -> Self {
            std::env::set_var("FORKDECK_FORGE_FIXTURE_SECRET", "fixture-secret");
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let provider = format!("http://{}",listener.local_addr().unwrap());
            let fake = Arc::new(Fake { calls: AtomicUsize::new(0), fail, echo_metadata:AtomicBool::new(false), codes: Mutex::new(HashMap::new()) });
            let routes = Router::new().route("/login/oauth/authorize",get(authorize)).route("/oauth/authorize",get(authorize))
                .route("/login/oauth/access_token",post(token)).route("/oauth/token",post(token))
                .route("/{provider}/repos",get(listing)).route("/api/v4/{provider}",get(listing)).with_state(fake.clone());
            let provider_task = tokio::spawn(async move { axum::serve(listener,routes).await.unwrap(); });
            let mut cfg = crate::config::Config::default();
            if enabled {
                cfg.web.forges = toml::from_str(&format!(r#"
[github]
client_id = "github-client"
client_secret_env = "FORKDECK_FORGE_FIXTURE_SECRET"
redirect_url = "http://127.0.0.1/forges/github/callback"
private = true
base_url = "{provider}"
[gitlab]
client_id = "gitlab-client"
client_secret_env = "FORKDECK_FORGE_FIXTURE_SECRET"
redirect_url = "http://127.0.0.1/forges/gitlab/callback"
base_url = "{provider}"
"#)).unwrap();
            }
            for (provider, host) in [("github","github.com"),("gitlab","gitlab.com")] {
                let mut repo: crate::config::RepoCfg = toml::from_str(&format!("provider = {provider:?}\nrepo = \"owner/already\"\npath = \"/unused\"\n")).unwrap();
                repo.host = host.into();
                cfg.repos.push(repo);
            }
            let hub = crate::orchestrator::Hub::new(cfg,tokio::runtime::Handle::current());
            let app = App { hub, feed: Arc::new(crate::remote::feed::Feed::new()), loopback: true, auth: None, writes: Default::default(), forges: Default::default() };
            let mut router = super::super::http::router(app.clone());
            if viewer { router = router.layer(axum::Extension(super::super::auth::Principal { subject:"viewer".into(),name:"Viewer".into(),groups:vec![],role:"viewer".into() })); }
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}",listener.local_addr().unwrap());
            let task = tokio::spawn(async move { axum::serve(listener,router).await.unwrap(); });
            Self { app, fake, client: reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap(), base, tasks:vec![task,provider_task] }
        }
        async fn flow(&self, provider: &str) -> (String,String,String) {
            let response = self.client.get(format!("{}/forges/{provider}/connect",self.base)).send().await.unwrap();
            assert_eq!(response.status(),StatusCode::SEE_OTHER);
            let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
            for flag in ["forkdeck_forge=", "Path=/forges", "HttpOnly", "SameSite=Lax", "Max-Age=300"] { assert!(cookie.contains(flag)); }
            assert!(!cookie.contains("Secure"));
            let cookie = cookie.split(';').next().unwrap().to_string();
            let authorize = response.headers()[header::LOCATION].to_str().unwrap();
            let url = reqwest::Url::parse(authorize).unwrap();
            let q: HashMap<_,_> = url.query_pairs().into_owned().collect();
            assert_eq!(q["scope"],if provider == "github" {"repo"} else {"read_api"});
            assert_eq!(q["state"].len(),43);
            let response = self.client.get(authorize).send().await.unwrap();
            let callback = reqwest::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
            (format!("{}/forges/{provider}/callback?{}",self.base,callback.query().unwrap()),cookie,q["state"].clone())
        }
    }
    #[tokio::test]
    async fn both_providers_pkce_pagination_and_browser_binding() {
        let f = Fixture::new(false,true,false).await;
        let providers: Value = f.client.get(format!("{}/api/v1/forges",f.base)).send().await.unwrap().json().await.unwrap();
        assert_eq!(providers,json!({"providers":[{"id":"github","host":"github.com"},{"id":"gitlab","host":"gitlab.com"}],"can_add":true}));
        for provider in ["github","gitlab"] {
            let (url,cookie,_) = f.flow(provider).await;
            let response = f.client.get(&url).header(header::COOKIE,&cookie).send().await.unwrap();
            assert_eq!(response.status(),StatusCode::SEE_OTHER);
            let api_cookie = response.headers().get_all(header::SET_COOKIE).iter().find_map(|h| { let h=h.to_str().unwrap(); h.contains("Path=/api/v1").then_some(h) }).expect("picker binding must reach API routes");
            assert!(api_cookie.starts_with(&cookie));
            let id = response.headers()[header::LOCATION].to_str().unwrap().strip_prefix("/repos/add/").unwrap();
            let endpoint = format!("{}/api/v1/forges/pickers/{id}",f.base);
            let response = f.client.get(&endpoint).header(header::COOKIE,&cookie).send().await.unwrap();
            assert_eq!(response.status(),StatusCode::OK);
            let body: Value = response.json().await.unwrap();
            assert_eq!(body["provider"],provider);
            assert!(body["expires_unix"].as_i64().unwrap() > now());
            assert_eq!(body["repos"].as_array().unwrap().len(),2);
            assert_eq!(body["repos"][0],json!({"repo":"owner/already","private":true,"default_branch":"main","description":"Real metadata","configured":true}));
            assert_eq!(body["repos"][1]["configured"],false);
            assert_eq!(f.client.get(&endpoint).header(header::COOKIE,"forkdeck_forge=other").send().await.unwrap().status(),StatusCode::NOT_FOUND);
            assert_eq!(f.client.get(url).header(header::COOKIE,&cookie).send().await.unwrap().status(),StatusCode::BAD_REQUEST);
            assert_eq!(f.fake.calls.load(Ordering::SeqCst),if provider == "github" {1} else {2});
            let raw_cookie = cookie.split_once('=').unwrap().1;
            let selected = picked(&f.app,id,raw_cookie,"owner/new").unwrap();
            assert_eq!(selected.repo,"owner/new");
            assert_eq!(selected.default_branch,"main");
            assert_eq!(super::id(selected.provider),provider);
            assert_eq!(selected.host,body["host"].as_str().unwrap());
            assert!(picked(&f.app,id,raw_cookie,"owner/not-listed").is_none());
            f.app.forges.lock().unwrap().pickers.iter_mut().find(|p| p.id == id).unwrap().expires = now()-1;
            assert_eq!(f.client.get(&endpoint).header(header::COOKIE,&cookie).send().await.unwrap().status(),StatusCode::NOT_FOUND);
        }
    }
    #[tokio::test]
    async fn invalid_states_never_contact_provider() {
        let f = Fixture::new(false,true,false).await;
        for kind in ["unknown","expired","cookie","provider"] {
            let (url,cookie,state) = f.flow("github").await;
            let url = match kind {
                "unknown" => format!("{}/forges/github/callback?code=code&state=unknown",f.base),
                "provider" => url.replace("/github/callback","/gitlab/callback"),
                _ => url,
            };
            if kind == "expired" { f.app.forges.lock().unwrap().pending.iter_mut().find(|p| p.state == state).unwrap().expires = now()-1; }
            let cookie = if kind == "cookie" { "forkdeck_forge=wrong" } else { &cookie };
            assert_eq!(f.client.get(url).header(header::COOKIE,cookie).send().await.unwrap().status(),StatusCode::BAD_REQUEST, "{kind}");
            assert_eq!(f.fake.calls.load(Ordering::SeqCst),0);
        }
    }
    #[tokio::test]
    async fn listing_errors_do_not_leak_provider_tokens() {
        let f = Fixture::new(true,true,false).await;
        let (url,cookie,_) = f.flow("github").await;
        let response = f.client.get(url).header(header::COOKIE,cookie).send().await.unwrap();
        assert_eq!(response.status(),StatusCode::BAD_GATEWAY);
        let body = response.text().await.unwrap();
        assert!(body.contains("listing") && body.contains("500"));
        assert!(!body.contains(TOKEN));
        assert!(!f.app.hub.state.lock().unwrap().log.iter().any(|line| line.msg.contains(TOKEN)));
        assert!(!serde_json::to_string(&f.app.audit().await.unwrap()).unwrap().contains(TOKEN));
    }
    #[tokio::test]
    async fn absent_configuration_is_404_and_viewers_cannot_connect() {
        let absent = Fixture::new(false,false,false).await;
        for path in ["/forges/github/connect","/forges/github/callback?code=x&state=x","/api/v1/forges","/api/v1/forges/pickers/missing"] {
            assert_eq!(absent.client.get(format!("{}{path}",absent.base)).send().await.unwrap().status(),StatusCode::NOT_FOUND,"{path}");
        }
        let viewer = Fixture::new(false,true,true).await;
        for path in ["/forges/github/connect","/forges/github/callback?code=x&state=x"] {
            assert_eq!(viewer.client.get(format!("{}{path}",viewer.base)).send().await.unwrap().status(),StatusCode::FORBIDDEN);
        }
        let body: Value = viewer.client.get(format!("{}/api/v1/forges",viewer.base)).send().await.unwrap().json().await.unwrap();
        assert_eq!(body["can_add"],false);
    }
    #[tokio::test]
    async fn pending_flows_are_bounded_secure_and_demo_is_refused() {
        let f = Fixture::new(false,true,false).await;
        let mut app = f.app.clone();
        Arc::make_mut(&mut app.hub.cfg).web.forges.github.as_mut().unwrap().redirect_url = "https://forkdeck.example/forges/github/callback".into();
        let mut first = String::new();
        for i in 0..65 {
            let response = connect(State(app.clone()),Path("github".into()),None).await;
            assert_eq!(response.status(),StatusCode::SEE_OTHER);
            assert!(response.headers()[header::SET_COOKIE].to_str().unwrap().contains("; Secure"));
            if i == 0 { first = app.forges.lock().unwrap().pending[0].state.clone(); }
        }
        assert_eq!(app.forges.lock().unwrap().pending.len(),64);
        assert!(app.forges.lock().unwrap().pending.iter().all(|p| p.state != first));
        Arc::make_mut(&mut app.hub.cfg).demo = true;
        let error = super::super::http::serve(app.hub,app.feed).await.unwrap_err();
        assert!(error.to_string().contains("--demo"));
    }

    #[tokio::test]
    async fn a_token_echoed_in_successful_metadata_never_enters_a_picker() {
        let f = Fixture::new(false,true,false).await;
        f.fake.echo_metadata.store(true,Ordering::SeqCst);
        let (url,cookie,_) = f.flow("github").await;
        let response = f.client.get(url).header(header::COOKIE,&cookie).send().await.unwrap();
        assert_eq!(response.status(),StatusCode::SEE_OTHER);
        let id = response.headers()[header::LOCATION].to_str().unwrap().strip_prefix("/repos/add/").unwrap();
        let response = f.client.get(format!("{}/api/v1/forges/pickers/{id}",f.base)).header(header::COOKIE,&cookie).send().await.unwrap();
        assert!(!response.text().await.unwrap().contains(TOKEN));
        assert!(f.app.forges.lock().unwrap().pickers.iter().all(|p| !serde_json::to_string(&p.repos).unwrap().contains(TOKEN)));
    }

}
