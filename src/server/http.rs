use super::{auth::{self, Auth}, view, wire};
use crate::{orchestrator::Hub, remote::feed::{Feed, Numbered}};
use axum::{
    body::Body,
    extract::{ws::{Message, WebSocket, WebSocketUpgrade}, Path, Query, State},
    http::{header, HeaderMap, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::broadcast::{self, error::RecvError};

/// Loopback host checks also protect an unauthenticated local server from DNS rebinding.
#[derive(Clone)]
pub(super) struct App { pub hub: Hub, pub feed: Arc<Feed>, pub(super) loopback: bool, pub auth: Option<Arc<Auth>>, pub writes: Arc<tokio::sync::Mutex<super::write::Writes>>, pub forges: Arc<std::sync::Mutex<super::forges::Forges>> }

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
    if app.loopback && app.auth.is_none() && !req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).is_some_and(local_host) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let protected = app.auth.is_some();
    let mut response = next.run(req).await;
    if protected { response.headers_mut().insert(header::CACHE_CONTROL, "no-store".parse().unwrap()); }
    response
}

async fn authenticated(State(app): State<App>, mut req: Request<Body>, next: Next) -> Response {
    if let Some(auth) = &app.auth {
        match auth.session(req.headers()).await {
            Ok(Some(principal)) => { req.extensions_mut().insert(principal); }
            Ok(None) => {
                // Only a page load starts a sign-in. A browser's subresource request (the favicon on
                // the signed-out page) would otherwise replace the flow the user is about to start.
                let subresource = req.headers().get("sec-fetch-mode").is_some_and(|m| m != "navigate");
                return if subresource || req.uri().path() == "/api" || req.uri().path().starts_with("/api/") {
                    StatusCode::UNAUTHORIZED.into_response()
                } else { axum::response::Redirect::to("/auth/login").into_response() };
            }
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        }
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
async fn doctor(State(app): State<App>) -> Response {
    let mut value = super::operations::doctor(&app.hub).await;
    match app.audit().await {
        Ok(audit) => value["audit"] = serde_json::json!(audit),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    Json(value).into_response()
}
async fn settings(State(app): State<App>) -> Json<serde_json::Value> { Json(super::operations::settings(&app.hub)) }

#[derive(Deserialize)]
struct Resume { since: Option<u64> }

async fn live(State(app): State<App>, Query(resume): Query<Resume>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    // Browsers let any page open a WebSocket to any host, so the Origin is checked here. Behind
    // OIDC the browser sees the configured public origin; locally it sees this server's Host.
    if let Some(origin) = headers.get(header::ORIGIN) {
        let allowed = match &app.auth {
            Some(auth) => origin.to_str().ok().is_some_and(|o| o == auth.origin()),
            None => {
                let authority = origin.to_str().ok().and_then(|s| s.parse::<axum::http::Uri>().ok()).and_then(|uri| uri.authority().map(|a| a.as_str().to_owned()));
                authority.as_deref().zip(headers.get(header::HOST).and_then(|h| h.to_str().ok())).is_some_and(|(a, h)| a.eq_ignore_ascii_case(h))
            }
        };
        if headers.get_all(header::ORIGIN).iter().count() != 1 || !allowed {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    let session = app.auth.as_ref().and_then(|auth| auth.session_key(&headers).map(|key| (auth.clone(), key)));
    let (backlog, rx) = app.feed.subscribe(resume.since.unwrap_or(0));
    ws.on_upgrade(move |socket| live_socket(app, socket, backlog, rx, session))
}

async fn next_live(rx: &mut broadcast::Receiver<Numbered>) -> Option<Numbered> {
    match rx.recv().await {
        Ok(event) => Some(event),
        Err(RecvError::Lagged(_)) => Some((0, forkdeck_remote_proto::Event::Resync)),
        Err(RecvError::Closed) => None,
    }
}

/// A signed-in socket closes within a second of sign-out or expiry, like the session it rode in on.
async fn live_socket(app: App, mut socket: WebSocket, backlog: Vec<Numbered>, mut rx: broadcast::Receiver<Numbered>, session: Option<(Arc<Auth>, String)>) {
    let signed_in = || async { match &session { Some((auth, key)) => matches!(auth.session_hash(key.clone()).await, Ok(Some(_))), None => true } };
    for (seq, event) in backlog {
        if socket.send(Message::Binary(wire::event(&app.hub, seq, &event).into())).await.is_err() { return; }
    }
    let every = Duration::from_secs(10);
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut check = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                _ => {},
            },
            _ = ping.tick() => {
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() { return; }
            }
            _ = check.tick(), if session.is_some() => {
                if !signed_in().await {
                    let _ = socket.send(Message::Close(None)).await;
                    return;
                }
            }
            event = next_live(&mut rx) => {
                let Some((seq, event)) = event else {
                    let _ = socket.send(Message::Close(None)).await;
                    return;
                };
                if socket.send(Message::Binary(wire::event(&app.hub, seq, &event).into())).await.is_err() { return; }
            }
        }
    }
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
    // Not no-referrer: under it browsers send `Origin: null` on same-origin form posts, which the
    // sign-out Origin check must refuse. same-origin still sends nothing to other sites.
    h.insert("referrer-policy", "same-origin".parse().unwrap());
    h.insert("content-security-policy", "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'".parse().unwrap());
    response
}

pub(super) fn router(app: App) -> Router {
    let api = Router::new()
        .route("/v1/forges", get(super::forges::providers))
        .route("/v1/forges/pickers/{id}", get(super::forges::picker))
        .route("/v1/me", get(auth::whoami))
        .route("/v1/actions", post(super::write::action))
        .route("/v1/snapshot", get(snapshot))
        .route("/v1/jobs/{id}", get(detail))
        .route("/v1/plans", get(plans))
        .route("/v1/plans/{id}", get(plan))
        .route("/v1/live", get(live))
        .route("/v1/repos", get(repos))
        .route("/v1/deploys", get(deploys))
        .route("/v1/stats", get(stats))
        .route("/v1/doctor", get(doctor))
        .route("/v1/settings", get(settings))
        .fallback(|| async { StatusCode::NOT_FOUND })
        .layer(middleware::from_fn(no_store));
    let protected = Router::new().nest("/api", api)
        .route("/forges/{provider}/connect", get(super::forges::connect).layer(middleware::from_fn(no_store)))
        .route("/forges/{provider}/callback", get(super::forges::callback).layer(middleware::from_fn(no_store)))
        .fallback(assets)
        .layer(middleware::from_fn_with_state(app.clone(), authenticated));
    let routes = match &app.auth { Some(auth) => protected.merge(auth::routes(auth.clone())), None => protected };
    routes
        .layer(middleware::from_fn_with_state(app.clone(), same_machine))
        .with_state(app).layer(middleware::from_fn(security))
}

pub async fn serve(hub: Hub, feed: Arc<Feed>) -> anyhow::Result<()> {
    anyhow::ensure!(!hub.cfg.demo || (hub.cfg.web.forges.github.is_none() && hub.cfg.web.forges.gitlab.is_none()), "--demo refuses web.forges configuration");
    let at: SocketAddr = hub.cfg.web.address()?;
    let auth = match &hub.cfg.web.oidc {
        Some(cfg) => Some(Auth::new(cfg.clone(), hub.history.clone().ok_or_else(|| anyhow::anyhow!("OIDC requires durable history storage"))?).await?),
        None => None,
    };
    let listener = tokio::net::TcpListener::bind(at).await?;
    let local = listener.local_addr()?;
    let loopback = local.ip().is_loopback();
    println!("Forkdeck web UI: http://{local}/");
    if !loopback && auth.is_none() {
        eprintln!("warning: the web UI has no login and listens on {local}: anyone who can reach that address can read and operate this workspace. Keep it behind a firewall, VPN or an authenticating reverse proxy.");
    }
    axum::serve(listener, router(App { hub, feed, loopback, auth, writes: Default::default(), forges: Default::default() })).await?;
    Ok(())
}

#[cfg(test)]
pub(super) fn test_router(hub: Hub, feed: Arc<Feed>) -> Router {
    router(App { hub, feed, loopback: true, auth: None, writes: Default::default(), forges: Default::default() })
}

#[cfg(test)]
pub(crate) async fn test_post(hub: Hub, body: serde_json::Value, key: &str) -> (StatusCode, serde_json::Value) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = router(App { hub, feed: Arc::new(Feed::new()), loopback: true, auth: None, writes: Default::default(), forges: Default::default() });
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    let result = reqwest::Client::new().post(format!("{base}/api/v1/actions")).header("idempotency-key",key).json(&body).send().await.unwrap();
    let status = result.status();
    let body = result.json().await.unwrap();
    task.abort();
    (status, body)
}

#[cfg(test)]
pub(super) fn auth_test_router(auth: Arc<Auth>, seeded: bool) -> Router {
    let mut cfg = crate::config::Config { demo: true, ..Default::default() };
    if seeded { super::seed::configure(&mut cfg); }
    let hub = Hub::new(cfg, tokio::runtime::Handle::current());
    if seeded { super::seed::seed_demo(&hub).unwrap(); }
    let feed = Arc::new(Feed::new());
    tokio::spawn(feed.clone().run(hub.clone(), Duration::from_millis(200)));
    router(App { hub, feed, loopback: true, auth: Some(auth), writes: Default::default(), forges: Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::{connect_async, tungstenite::{client::IntoClientRequest, Message}};
    use super::super::wire::board_generated::forkdeck::web as fb;

    async fn binary(ws: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>) -> Vec<u8> {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match ws.next().await.unwrap().unwrap() {
                    Message::Binary(bytes) => return bytes.to_vec(),
                    Message::Ping(_) | Message::Pong(_) => {},
                    other => panic!("expected a binary frame, got {other:?}"),
                }
            }
        }).await.unwrap()
    }

    #[test]
    fn only_this_machines_names_count_as_local() {
        for host in ["127.0.0.1:7420", "localhost:7420", "LOCALHOST", "[::1]:7420", "127.0.0.2"] { assert!(local_host(host), "{host}"); }
        for host in ["evil.example:7420", "evil.example", "10.0.0.5:7420", "localhost.evil.example", "[::1].evil:80"] { assert!(!local_host(host), "{host}"); }
    }

    #[tokio::test]
    async fn snapshot_and_live_resume_without_a_login() {
        let mut cfg = crate::config::Config { demo: true, ..Default::default() };
        crate::server::seed::configure(&mut cfg);
        let hub = Hub::new(cfg, tokio::runtime::Handle::current());
        crate::server::seed::seed_demo(&hub).unwrap();
        let feed = Arc::new(Feed::new());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = router(App { hub: hub.clone(), feed: feed.clone(), loopback: true, auth: None, writes: Default::default(), forges: Default::default() });
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
        for path in ["", "api/v1/snapshot", "api/v1/live"] {
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
        let live = format!("{}/api/v1/live", base.replace("http://", "ws://"));
        let since = frame.seq();
        let (mut ws, _) = connect_async(format!("{live}?since={since}")).await.unwrap();
        ws.send(Message::Text("ignored".into())).await.unwrap();
        ws.send(Message::Binary(vec![1,2,3].into())).await.unwrap();
        ws.send(Message::Ping(vec![7].into())).await.unwrap();
        let pong = tokio::time::timeout(Duration::from_secs(2), ws.next()).await.unwrap().unwrap().unwrap();
        assert_eq!(pong, Message::Pong(vec![7].into()));
        hub.job(72, |j| j.title = "updated through the hub".into());
        feed.tick(&hub);
        let bytes = binary(&mut ws).await;
        let changed = fb::root_as_frame(&bytes).unwrap();
        assert!(changed.seq() > since);
        let job = changed.body_as_job_changed().unwrap().job().unwrap();
        assert_eq!((job.id(), job.title()), (72, Some("updated through the hub")));
        ws.close(None).await.unwrap();

        for n in 0..3 {
            hub.job(72, |j| j.title = format!("backlog {n}"));
            feed.tick(&hub);
        }
        let expected = feed.subscribe(since).0;
        assert!(expected.len() >= 4);
        let (mut ws, _) = connect_async(format!("{live}?since={since}")).await.unwrap();
        let mut previous = since;
        for (seq, _) in expected {
            let bytes = binary(&mut ws).await;
            let frame = fb::root_as_frame(&bytes).unwrap();
            assert_eq!(frame.seq(), seq);
            assert!(frame.seq() > previous);
            previous = frame.seq();
        }
        ws.close(None).await.unwrap();
        // Startup timestamps make 1 an expired cursor; a future cursor covers daemon restarts.
        for since in [1, u64::MAX] {
            let (mut ws, _) = connect_async(format!("{live}?since={since}")).await.unwrap();
            let bytes = binary(&mut ws).await;
            assert_eq!(fb::root_as_frame(&bytes).unwrap().body_type(), fb::Body::Resync);
            ws.close(None).await.unwrap();
        }
        for origin in ["http://evil.test", "null", "http://127.0.0.1:1", "invalid"] {
            let mut request = live.clone().into_client_request().unwrap();
            request.headers_mut().insert(header::ORIGIN, origin.parse().unwrap());
            let err = connect_async(request).await.unwrap_err();
            match err {
                tokio_tungstenite::tungstenite::Error::Http(response) => assert_eq!(response.status(), StatusCode::FORBIDDEN),
                other => panic!("expected a forbidden handshake, got {other}"),
            }
        }
        let mut request = live.clone().into_client_request().unwrap();
        request.headers_mut().insert(header::ORIGIN, base.parse().unwrap());
        let (mut ws, _) = connect_async(request).await.unwrap();
        let ping = tokio::time::timeout(Duration::from_secs(12), ws.next()).await.unwrap().unwrap().unwrap();
        assert!(matches!(ping, Message::Ping(_)));
        ws.close(None).await.unwrap();
        assert_eq!(client.get(format!("{base}/api/v1/events")).send().await.unwrap().status(), StatusCode::NOT_FOUND);
        server.abort();
    }

    #[tokio::test]
    async fn a_lagged_receiver_sends_resync_and_keeps_receiving() {
        let hub = Hub::new(crate::config::Config::default(), tokio::runtime::Handle::current());
        let feed = Feed::new();
        let (_, mut rx) = feed.subscribe(0);
        for n in 0..1100 {
            hub.state.lock().unwrap().automation = n % 2 == 0;
            feed.tick(&hub);
        }
        let (seq, event) = next_live(&mut rx).await.unwrap();
        let bytes = wire::event(&hub, seq, &event);
        assert_eq!(fb::root_as_frame(&bytes).unwrap().body_type(), fb::Body::Resync);
        assert!(matches!(next_live(&mut rx).await, Some((_, forkdeck_remote_proto::Event::Automation { .. }))));
        let (tx, mut rx) = tokio::sync::broadcast::channel(1);
        drop(tx);
        assert!(next_live(&mut rx).await.is_none());
    }

}
