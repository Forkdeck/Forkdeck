use super::{view, wire};
use crate::{orchestrator::Hub, remote::feed::{Feed, Numbered}};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, Request, StatusCode},
    middleware::{self, Next},
    response::{sse::{Event, KeepAlive, Sse}, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use std::{collections::VecDeque, convert::Infallible, net::SocketAddr, sync::Arc, time::Duration};

/// There is no login: whoever reaches the address sees everything. `loopback` says the server
/// listens on this machine only, where the one remaining risk is another site's page reaching it.
#[derive(Clone)]
struct App { hub: Hub, feed: Arc<Feed>, loopback: bool }

#[derive(rust_embed::RustEmbed)]
#[folder = "web/dist/"]
struct Assets;

/// On a loopback address, a page from another site can still reach the server by pointing its own
/// name at 127.0.0.1 (DNS rebinding); its requests then carry that name as their Host. Only the
/// machine's own names are answered.
fn local_host(host: &str) -> bool {
    let port = |p: &str| p.is_empty() || p.strip_prefix(':').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    let name = match host.strip_prefix('[') {
        Some(rest) => match rest.split_once(']') { Some((name, after)) if port(after) => name, _ => return false },
        None => host.rsplit_once(':').map_or(host, |(name, p)| if port(&format!(":{p}")) { name } else { host }),
    };
    name.eq_ignore_ascii_case("localhost") || name.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

async fn same_machine(State(app): State<App>, req: Request<Body>, next: Next) -> Response {
    if app.loopback && !req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).is_some_and(local_host) {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(req).await
}

async fn no_store(req: Request<Body>, next: Next) -> Response {
    let mut response = next.run(req).await;
    response.headers_mut().insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

async fn snapshot(State(app): State<App>) -> impl IntoResponse { ([(header::CONTENT_TYPE, "application/octet-stream")], wire::board(&app.hub, &app.feed)) }
async fn detail(State(app): State<App>, Path(id): Path<u32>) -> Response {
    match view::detail(&app.hub, &app.feed, id) { Some(v) => Json(v).into_response(), None => StatusCode::NOT_FOUND.into_response() }
}
async fn plans(State(app): State<App>) -> Json<Vec<serde_json::Value>> { Json(view::plans(&app.hub)) }
async fn plan(State(app): State<App>, Path(id): Path<String>) -> Response {
    match view::plans(&app.hub).into_iter().find(|p| p["id"].as_str().map(str::to_owned).unwrap_or_else(|| p["id"].to_string()) == id) { Some(p) => Json(p).into_response(), None => StatusCode::NOT_FOUND.into_response() }
}

async fn repos(State(app): State<App>) -> Json<serde_json::Value> { Json(super::operations::repos(&app.hub)) }
async fn deploys(State(app): State<App>) -> Json<serde_json::Value> { Json(super::operations::deploys(&app.hub)) }
#[derive(Deserialize)]
struct StatsWindow { days: Option<u32> }
async fn stats(State(app): State<App>, Query(window): Query<StatsWindow>) -> Json<serde_json::Value> { Json(super::operations::stats(&app.hub, window.days.unwrap_or(7).clamp(1,3650))) }
async fn doctor(State(app): State<App>) -> Json<serde_json::Value> { Json(super::operations::doctor(&app.hub).await) }
async fn settings(State(app): State<App>) -> Json<serde_json::Value> { Json(super::operations::settings(&app.hub)) }

#[derive(Deserialize)]
struct Cursor { since: Option<u64> }

async fn events(State(app): State<App>, Query(cursor): Query<Cursor>, headers: HeaderMap) -> impl IntoResponse {
    let since = headers.get("last-event-id").and_then(|s| s.to_str().ok()).and_then(|s| s.parse().ok()).or(cursor.since).unwrap_or(0);
    let (backlog, rx) = app.feed.subscribe(since);
    let stream = futures::stream::unfold((VecDeque::from(backlog), rx), |(mut queue, mut rx)| async move {
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
        Some((Ok::<_, Infallible>(event), (queue, rx)))
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
        .route("/v1/repos", get(repos))
        .route("/v1/deploys", get(deploys))
        .route("/v1/stats", get(stats))
        .route("/v1/doctor", get(doctor))
        .route("/v1/settings", get(settings))
        .fallback(|| async { StatusCode::NOT_FOUND })
        .layer(middleware::from_fn(no_store));
    Router::new().nest("/api", api)
        .fallback(assets)
        .layer(middleware::from_fn_with_state(app.clone(), same_machine))
        .with_state(app).layer(middleware::from_fn(security))
}

pub async fn serve(hub: Hub, feed: Arc<Feed>) -> anyhow::Result<()> {
    let at: SocketAddr = hub.cfg.web.address()?;
    let listener = tokio::net::TcpListener::bind(at).await?;
    let local = listener.local_addr()?;
    let loopback = local.ip().is_loopback();
    println!("Forkdeck web UI: http://{local}/");
    if !loopback {
        eprintln!("warning: the web UI has no login and listens on {local}: anyone who can reach that address sees every job, log and plan. Keep it behind a firewall, VPN or an authenticating reverse proxy.");
    }
    axum::serve(listener, router(App { hub, feed, loopback })).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_this_machines_names_count_as_local() {
        for host in ["127.0.0.1:7420", "localhost:7420", "LOCALHOST", "[::1]:7420", "127.0.0.2"] { assert!(local_host(host), "{host}"); }
        for host in ["evil.example:7420", "evil.example", "10.0.0.5:7420", "localhost.evil.example", "[::1].evil:80"] { assert!(!local_host(host), "{host}"); }
    }

    #[tokio::test]
    async fn snapshot_and_sse_resume_without_a_login() {
        let mut cfg = crate::config::Config { demo: true, ..Default::default() };
        crate::server::seed::configure(&mut cfg);
        let hub = Hub::new(cfg, tokio::runtime::Handle::current());
        crate::server::seed::seed_demo(&hub).unwrap();
        let feed = Arc::new(Feed::new());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = router(App { hub: hub.clone(), feed: feed.clone(), loopback: true });
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
        // A page from another site that rebinds its own name to 127.0.0.1 is refused.
        for path in ["", "api/v1/snapshot", "api/v1/events"] {
            assert_eq!(client.get(format!("{base}/{path}")).header(header::HOST, "evil.example").send().await.unwrap().status(), StatusCode::FORBIDDEN);
        }
        let snapshot = client.get(format!("{base}/api/v1/snapshot")).send().await.unwrap();
        assert_eq!(snapshot.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(snapshot.status(), StatusCode::OK);
        assert_eq!(snapshot.headers()[header::CONTENT_TYPE], "application/octet-stream");
        let bytes = snapshot.bytes().await.unwrap();
        let frame = super::super::wire::board_generated::forkdeck::web::root_as_frame(&bytes).unwrap();
        let board = frame.body_as_snapshot().unwrap();
        let expected = view::snapshot(&hub, &feed);
        let jobs = board.jobs().unwrap();
        assert_eq!(jobs.len(), expected["jobs"].as_array().unwrap().len());
        for (job, expected) in jobs.iter().zip(expected["jobs"].as_array().unwrap()) {
            assert_eq!(job.id() as u64, expected["id"].as_u64().unwrap());
            assert_eq!(job.lane(), expected["lane"].as_str());
            assert_eq!(job.state(), expected["state"].as_str());
        }
        for (endpoint, field) in [("repos", ""), ("deploys", "repositories"), ("stats", "summary"), ("doctor", "checks"), ("settings", "agents")] {
            let response = client.get(format!("{base}/api/v1/{endpoint}")).send().await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{endpoint}");
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            let body: serde_json::Value = response.json().await.unwrap();
            if field.is_empty() { assert!(!body.as_array().unwrap().is_empty()); }
            else { assert!(!body[field].is_null(), "{endpoint}: {body}"); }
            assert_eq!(client.post(format!("{base}/api/v1/{endpoint}")).send().await.unwrap().status(), StatusCode::METHOD_NOT_ALLOWED);
        }
        let plans: serde_json::Value = client.get(format!("{base}/api/v1/plans/100")).send().await.unwrap().json().await.unwrap();
        assert_eq!(plans["plan"]["revision"], 3);
        assert_eq!(plans["plan"]["reviewed"], 3);
        assert_eq!(plans["plan"]["accepted"], 3);
        assert_eq!(plans["plan"]["published"], 3);
        assert!(plans["plan"]["draft"].as_str().unwrap().contains("forkdeck-tasks"));
        assert!(!plans["draft_display"].as_str().unwrap().contains("forkdeck-tasks"));
        assert!(plans["plan"]["review"].as_str().unwrap().contains("VERDICT: PASS"));
        hub.log("test", "resume me");
        feed.tick(&hub);
        let mut response = client.get(format!("{base}/api/v1/events")).header("last-event-id", frame.seq().to_string()).send().await.unwrap();
        let chunk = tokio::time::timeout(Duration::from_secs(2), response.chunk()).await.unwrap().unwrap().unwrap();
        let chunk = String::from_utf8(chunk.to_vec()).unwrap();
        assert!(chunk.contains("id:") && chunk.contains("resume me"), "{chunk}");
        let mut response = client.get(format!("{base}/api/v1/events")).header("last-event-id", "1").send().await.unwrap();
        let chunk = response.chunk().await.unwrap().unwrap();
        assert!(String::from_utf8(chunk.to_vec()).unwrap().contains("Resync"));
        server.abort();
    }
}
