use super::{auth::Auth, view};
use crate::{orchestrator::Hub, remote::feed::{Feed, Numbered}};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, Request, StatusCode},
    middleware::{self, Next},
    response::{sse::{Event, KeepAlive, Sse}, Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Form, Json, Router,
};
use serde::Deserialize;
use std::{collections::VecDeque, convert::Infallible, sync::{Arc, Mutex}, time::Duration};

#[derive(Clone)]
struct App { hub: Hub, feed: Arc<Feed>, auth: Arc<Mutex<Auth>> }

#[derive(rust_embed::RustEmbed)]
#[folder = "web/dist/"]
struct Assets;

fn session(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::COOKIE)?.to_str().ok()?.split(';').find_map(|part| part.trim().strip_prefix("forkdeck_session="))
}

async fn authenticated(State(app): State<App>, req: Request<Body>, next: Next) -> Response {
    if !session(req.headers()).is_some_and(|s| app.auth.lock().unwrap().valid(s)) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "Login using the URL printed by forkdeck serve"}))).into_response();
    }
    let mut response = next.run(req).await;
    response.headers_mut().insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

#[derive(Deserialize)]
struct Login { code: String }

async fn login(Query(login): Query<Login>) -> Html<String> {
    // Only hex tokens enter markup. Exchanging is a POST, never a state-changing GET.
    let code = if login.code.len() == 64 && login.code.bytes().all(|b| b.is_ascii_hexdigit()) { login.code } else { String::new() };
    Html(format!("<!doctype html><html lang=\"en\"><head><meta name=\"referrer\" content=\"same-origin\"><title>Forkdeck login</title></head><body><form action=\"/auth/exchange\" method=\"post\"><input type=\"hidden\" name=\"code\" value=\"{code}\"><button>Open Forkdeck</button></form><script src=\"/auth/login.js\"></script></body></html>"))
}

async fn exchange(State(app): State<App>, headers: HeaderMap, Form(login): Form<Login>) -> Response {
    if let Some(origin) = headers.get(header::ORIGIN) {
        let expected = headers.get(header::HOST).and_then(|h| h.to_str().ok()).map(|h| format!("http://{h}"));
        if origin.to_str().ok() != expected.as_deref() { return StatusCode::FORBIDDEN.into_response(); }
    }
    let Some(token) = app.auth.lock().unwrap().exchange(&login.code) else { return StatusCode::UNAUTHORIZED.into_response() };
    let mut response = Redirect::to("/").into_response();
    response.headers_mut().insert(header::SET_COOKIE, format!("forkdeck_session={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=43200").parse().unwrap());
    response
}

async fn snapshot(State(app): State<App>) -> Json<serde_json::Value> { Json(view::snapshot(&app.hub, &app.feed)) }
async fn detail(State(app): State<App>, Path(id): Path<u32>) -> Response {
    match view::detail(&app.hub, &app.feed, id) { Some(v) => Json(v).into_response(), None => StatusCode::NOT_FOUND.into_response() }
}
async fn plans(State(app): State<App>) -> Json<Vec<serde_json::Value>> { Json(view::plans(&app.hub)) }
async fn plan(State(app): State<App>, Path(id): Path<String>) -> Response {
    match view::plans(&app.hub).into_iter().find(|p| p["id"].as_str().map(str::to_owned).unwrap_or_else(|| p["id"].to_string()) == id) { Some(p) => Json(p).into_response(), None => StatusCode::NOT_FOUND.into_response() }
}

#[derive(Deserialize)]
struct Cursor { since: Option<u64> }

async fn events(State(app): State<App>, Query(cursor): Query<Cursor>, headers: HeaderMap) -> impl IntoResponse {
    let since = headers.get("last-event-id").and_then(|s| s.to_str().ok()).and_then(|s| s.parse().ok()).or(cursor.since).unwrap_or(0);
    let (backlog, rx) = app.feed.subscribe(since);
    let token = session(&headers).unwrap_or_default().to_string();
    let stream = futures::stream::unfold((VecDeque::from(backlog), rx, app, token), |(mut queue, mut rx, app, token)| async move {
        if !app.auth.lock().unwrap().valid(&token) { return None; }
        let item: Option<Numbered> = if let Some(e) = queue.pop_front() { Some(e) } else {
            match tokio::time::timeout(Duration::from_secs(15), rx.recv()).await {
                Ok(Ok(e)) => Some(e),
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => return None,
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => Some((0, forkdeck_remote_proto::Event::Resync)),
                Err(_) => None,
            }
        };
        let event = match item {
            Some((seq, event)) => Event::default().id(seq.to_string()).json_data(event).unwrap(),
            None => Event::default().event("freshness").data("{}"),
        };
        Some((Ok::<_, Infallible>(event), (queue, rx, app, token)))
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(10)).text("heartbeat"))
}

async fn assets(req: Request<Body>) -> Response {
    let path = req.uri().path().trim_start_matches('/');
    let path = if path.is_empty() || !path.rsplit('/').next().unwrap_or_default().contains('.') { "index.html" } else { path };
    let Some(asset) = Assets::get(path) else { return StatusCode::NOT_FOUND.into_response() };
    let mut response = asset.data.into_owned().into_response();
    response.headers_mut().insert(header::CONTENT_TYPE, mime_guess::from_path(path).first_or_octet_stream().as_ref().parse().unwrap());
    response.headers_mut().insert(header::CACHE_CONTROL, if path.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" }.parse().unwrap());
    response
}

async fn security(req: Request<Body>, next: Next) -> Response {
    let mut response = next.run(req).await;
    let h = response.headers_mut();
    h.insert("x-content-type-options", "nosniff".parse().unwrap());
    h.insert("x-frame-options", "DENY".parse().unwrap());
    h.insert("referrer-policy", "no-referrer".parse().unwrap());
    h.insert("content-security-policy", "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'".parse().unwrap());
    response
}

fn router(app: App) -> Router {
    let api = Router::new()
        .route("/v1/snapshot", get(snapshot))
        .route("/v1/jobs/{id}", get(detail))
        .route("/v1/plans", get(plans))
        .route("/v1/plans/{id}", get(plan))
        .route("/v1/events", get(events))
        .fallback(|| async { StatusCode::NOT_FOUND })
        .layer(middleware::from_fn_with_state(app.clone(), authenticated));
    Router::new().nest("/api", api)
        .route("/auth/login", get(login))
        .route("/auth/login.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], "const data=new URLSearchParams(new FormData(document.querySelector('form')));history.replaceState(null,'','/auth/login');fetch('/auth/exchange',{method:'POST',body:data}).then(r=>{if(r.ok)location.replace('/');else document.body.textContent='Login expired or already used. Restart forkdeck serve for a new URL.'});") }))
        .route("/auth/exchange", post(exchange))
        .fallback(assets).with_state(app).layer(middleware::from_fn(security))
}

pub async fn serve(hub: Hub, feed: Arc<Feed>) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, hub.cfg.web.port)).await?;
    let (auth, code) = Auth::new()?;
    println!("Open Forkdeck (one use, expires in 2 minutes): http://{}/auth/login?code={}", listener.local_addr()?, code);
    axum::serve(listener, router(App { hub, feed, auth: Arc::new(Mutex::new(auth)) })).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn authenticated_snapshot_and_sse_resume() {
        let hub = Hub::new(crate::config::Config { demo: true, ..Default::default() }, tokio::runtime::Handle::current());
        view::seed_demo(&hub).unwrap();
        let feed = Arc::new(Feed::new());
        let (auth, code) = Auth::new().unwrap();
        let auth = Arc::new(Mutex::new(auth));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = router(App { hub: hub.clone(), feed: feed.clone(), auth });
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let index = client.get(format!("{base}/")).send().await.unwrap();
        assert_eq!(index.status(), StatusCode::OK);
        assert_eq!(index.headers()[header::CACHE_CONTROL], "no-cache");
        assert!(index.text().await.unwrap().contains("/assets/"));
        let asset = Assets::iter().find(|p| p.ends_with(".js")).unwrap();
        let asset = client.get(format!("{base}/{asset}")).send().await.unwrap();
        assert_eq!(asset.headers()[header::CACHE_CONTROL], "public, max-age=31536000, immutable");
        assert!(!asset.headers().contains_key("access-control-allow-origin"));
        for path in ["snapshot", "jobs/72", "plans", "plans/1", "events"] {
            assert_eq!(client.get(format!("{base}/api/v1/{path}")).send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
        }
        assert_eq!(client.post(format!("{base}/auth/exchange")).header(header::ORIGIN, "https://elsewhere.example").form(&[("code", &code)]).send().await.unwrap().status(), StatusCode::FORBIDDEN);
        let response = client.post(format!("{base}/auth/exchange")).form(&[("code", &code)]).send().await.unwrap();
        let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string();
        assert!(response.headers()[header::SET_COOKIE].to_str().unwrap().contains("HttpOnly; SameSite=Strict"));
        assert_eq!(client.post(format!("{base}/auth/exchange")).form(&[("code", &code)]).send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
        let snap: serde_json::Value = client.get(format!("{base}/api/v1/snapshot")).header(header::COOKIE, &cookie).send().await.unwrap().json().await.unwrap();
        assert!(snap["seq"].is_u64());
        assert!(snap["automation"].is_boolean());
        assert!(snap["agents"].is_object());
        assert!(snap["jobs"].as_array().unwrap().iter().any(|j| j["state"] == "unverified" && j["lane"] == "Attention"));
        hub.log("test", "resume me");
        feed.tick(&hub);
        let mut response = client.get(format!("{base}/api/v1/events")).header(header::COOKIE, &cookie).header("last-event-id", snap["seq"].to_string()).send().await.unwrap();
        let chunk = tokio::time::timeout(Duration::from_secs(2), response.chunk()).await.unwrap().unwrap().unwrap();
        let chunk = String::from_utf8(chunk.to_vec()).unwrap();
        assert!(chunk.contains("id:") && chunk.contains("resume me"), "{chunk}");
        let mut response = client.get(format!("{base}/api/v1/events")).header(header::COOKIE, &cookie).header("last-event-id", "1").send().await.unwrap();
        let chunk = response.chunk().await.unwrap().unwrap();
        assert!(String::from_utf8(chunk.to_vec()).unwrap().contains("Resync"));
        server.abort();
    }
}
