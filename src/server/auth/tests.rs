use super::*;
use axum::{extract::Form, routing::{get, post}};
use openidconnect::{core::CoreEdDsaPrivateSigningKey, JsonWebKeyId, PrivateSigningKey};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Provider {
    base: String,
    key: CoreEdDsaPrivateSigningKey,
    rotated: CoreEdDsaPrivateSigningKey,
    mode: &'static str,
    codes: Mutex<HashMap<String, (String, String)>>,
    jwks_reads: AtomicUsize,
    userinfo_reads: AtomicUsize,
}

async fn discovery(State(p): State<Arc<Provider>>) -> Json<Value> {
    Json(json!({"issuer":p.base,"authorization_endpoint":format!("{}/authorize",p.base),"token_endpoint":format!("{}/token",p.base),"userinfo_endpoint":format!("{}/userinfo",p.base),"jwks_uri":format!("{}/jwks",p.base),"response_types_supported":["code"],"subject_types_supported":["public"],"id_token_signing_alg_values_supported":["EdDSA"],"code_challenge_methods_supported":["S256"]}))
}
async fn jwks(State(p): State<Arc<Provider>>) -> Json<Value> {
    let reads = p.jwks_reads.fetch_add(1, Ordering::SeqCst);
    let key = if p.mode == "rotation" && reads > 0 { &p.rotated } else { &p.key };
    Json(json!({"keys":[key.as_verification_key()]}))
}
async fn authorize(State(p): State<Arc<Provider>>, Query(q): Query<HashMap<String,String>>) -> Redirect {
    assert_eq!(q["code_challenge_method"], "S256");
    assert_eq!(q["response_type"], "code");
    let code = random();
    p.codes.lock().await.insert(code.clone(), (q["nonce"].clone(), q["code_challenge"].clone()));
    let mut url = reqwest::Url::parse(&q["redirect_uri"]).unwrap();
    url.query_pairs_mut().append_pair("code", &code).append_pair("state", &q["state"]);
    Redirect::to(url.as_str())
}
async fn token(State(p): State<Arc<Provider>>, Form(q): Form<HashMap<String,String>>) -> Json<Value> {
    let (nonce, challenge) = p.codes.lock().await.remove(&q["code"]).unwrap();
    assert_eq!(hash(&q["code_verifier"]), challenge);
    assert_eq!(q["grant_type"], "authorization_code");
    let mut claims = json!({"iss":p.base,"sub":"user-1","aud":"forkdeck-test","iat":now(),"exp":now()+300,"nonce":nonce,"name":"Test member"});
    match p.mode {
        "audience" => claims["aud"] = json!("other-client"),
        "expired" => claims["exp"] = json!(now()-60),
        "issuer" => claims["iss"] = json!("https://wrong.example"),
        "nonce" => claims["nonce"] = json!("wrong"),
        _ => {}
    }
    match p.mode {
        "nested" => claims["realm_access"] = json!({"roles":["readers", "operators"]}),
        "missing" => claims["groups"] = json!(["outsiders"]),
        "userinfo" | "userinfo-subject" | "userinfo-missing" => {},
        "malformed" => claims["groups"] = json!("readers"),
        "capture-viewer" => claims["groups"] = json!(["readers"]),
        _ => claims["groups"] = json!(["readers", "admins"]),
    }
    let key = if p.mode == "rotation" { &p.rotated } else { &p.key };
    let kid = if p.mode == "rotation" { "rotated" } else { "initial" };
    let payload = format!("{}.{}", URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({"alg":"EdDSA","kid":kid})).unwrap()), URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap()));
    let mut signature = key.sign(&CoreJwsSigningAlgorithm::EdDsa, payload.as_bytes()).unwrap();
    if p.mode == "signature" { signature[0] ^= 1; }
    Json(json!({"access_token":"provider-access","token_type":"Bearer","id_token":format!("{payload}.{}",URL_SAFE_NO_PAD.encode(signature))}))
}
async fn userinfo(State(p): State<Arc<Provider>>, headers: HeaderMap) -> Json<Value> {
    assert_eq!(headers[header::AUTHORIZATION], "Bearer provider-access");
    p.userinfo_reads.fetch_add(1, Ordering::SeqCst);
    Json(match p.mode {
        "userinfo-subject" => json!({"sub":"another-user","groups":["readers"]}),
        "userinfo-missing" => json!({"sub":"user-1"}),
        _ => json!({"sub":"user-1","groups":["readers"]}),
    })
}
struct Fixture {
    auth: Arc<Auth>,
    provider: Arc<Provider>,
    base: String,
    client: reqwest::Client,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    path: std::path::PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) { for task in &self.tasks { task.abort(); } let _ = std::fs::remove_dir_all(&self.path); }
}
impl Fixture {
    async fn new(mode: &'static str) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let key = |kid: &str| {
            let pem = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap().serialize_pem();
            CoreEdDsaPrivateSigningKey::from_ed25519_pem(&pem, Some(JsonWebKeyId::new(kid.into()))).unwrap()
        };
        let provider = Arc::new(Provider { base: base.clone(), key: key("initial"), rotated: key("rotated"), mode, codes: Mutex::new(HashMap::new()), jwks_reads: AtomicUsize::new(0), userinfo_reads: AtomicUsize::new(0) });
        let routes = Router::new().route("/.well-known/openid-configuration", get(discovery)).route("/jwks", get(jwks)).route("/authorize", get(authorize)).route("/token", post(token)).route("/userinfo", get(userinfo)).with_state(provider.clone());
        let provider_task = tokio::spawn(async move { axum::serve(listener, routes).await.unwrap(); });
        let mut cfg: OidcCfg = serde_json::from_value(json!({"issuer":base,"client_id":"forkdeck-test","client_secret_env":"FORKDECK_TEST_SECRET","redirect_url":"https://forkdeck.example/auth/callback","groups_claim":if mode == "nested" {"realm_access.roles"} else {"groups"},"roles":{"viewer":["readers"],"operator":["operators"],"admin":["admins"]}})).unwrap();
        let capture = mode.starts_with("capture");
        if capture { cfg.redirect_url = std::env::var("FORKDECK_CAPTURE_ORIGIN").unwrap() + "/auth/callback"; }
        cfg.validate().unwrap();
        let path = std::env::temp_dir().join(format!("forkdeck-oidc-{}",random()));
        let history = Arc::new(History::open(&path.join("history.db")).unwrap());
        let auth = Auth::connect(cfg, history, "test-secret".into()).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let router = crate::server::http::auth_test_router(auth.clone(), capture);
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap(); });
        Self { auth, provider, base, client: reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap(), tasks: vec![task,provider_task], path }
    }
    async fn flow(&self) -> (String, String) {
        let login = self.client.get(format!("{}/auth/login", self.base)).send().await.unwrap();
        assert_eq!(login.status(), StatusCode::SEE_OTHER);
        let cookie = login.headers()[header::SET_COOKIE].to_str().unwrap();
        for flag in ["HttpOnly", "Secure", "SameSite=Lax", "Max-Age=300"] { assert!(cookie.contains(flag)); }
        let cookie = cookie.split(';').next().unwrap().to_owned();
        let redirect = self.client.get(login.headers()[header::LOCATION].to_str().unwrap()).send().await.unwrap();
        let public = reqwest::Url::parse(redirect.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        (format!("{}/auth/callback?{}",self.base,public.query().unwrap()), cookie)
    }
    async fn sign_in(&self) -> reqwest::Response {
        let (url, cookie) = self.flow().await;
        self.client.get(url).header(header::COOKIE,cookie).send().await.unwrap()
    }
    async fn session(&self) -> String {
        let response = self.sign_in().await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        response.headers().get_all(header::SET_COOKIE).iter().map(|v| v.to_str().unwrap()).find(|v| v.starts_with(SESSION)).unwrap().split(';').next().unwrap().to_string()
    }
}

#[tokio::test]
async fn wrong_state_and_tampered_flow_are_rejected() {
    let f = Fixture::new("normal").await;
    let (url,cookie) = f.flow().await;
    let mut wrong = reqwest::Url::parse(&url).unwrap();
    let code = wrong.query_pairs().find(|(k,_)| k == "code").unwrap().1.into_owned();
    wrong.set_query(None);
    wrong.query_pairs_mut().append_pair("code",&code).append_pair("state","wrong");
    assert_eq!(f.client.get(wrong).header(header::COOKIE,&cookie).send().await.unwrap().status(), StatusCode::FORBIDDEN);
    assert_eq!(f.client.get(&url).header(header::COOKIE,format!("{cookie}x")).send().await.unwrap().status(), StatusCode::FORBIDDEN);
    assert_eq!(f.client.get(url).header(header::COOKIE,cookie).send().await.unwrap().status(), StatusCode::SEE_OTHER);
}

async fn denied(mode: &'static str) {
    let f = Fixture::new(mode).await;
    let response = f.sign_in().await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN, "{mode}");
    assert!(!response.headers().get_all(header::SET_COOKIE).iter().any(|v| v.to_str().unwrap().starts_with(SESSION)));
}
#[tokio::test] async fn bad_signature() { denied("signature").await; }
#[tokio::test] async fn wrong_audience() { denied("audience").await; }
#[tokio::test] async fn expired_token() { denied("expired").await; }
#[tokio::test] async fn wrong_issuer() { denied("issuer").await; }
#[tokio::test] async fn wrong_nonce() { denied("nonce").await; }
#[tokio::test] async fn missing_group() { denied("missing").await; }
#[tokio::test] async fn malformed_group() { denied("malformed").await; }
#[tokio::test] async fn userinfo_subject_must_match() { denied("userinfo-subject").await; }
#[tokio::test] async fn userinfo_needs_groups() { denied("userinfo-missing").await; }

#[tokio::test]
async fn nested_group_claim_and_userinfo_fallback_and_key_rotation() {
    for (mode, role) in [("nested","operator"),("userinfo","viewer"),("rotation","admin")] {
        let f = Fixture::new(mode).await;
        let cookie = f.session().await;
        let response = f.client.get(format!("{}/api/v1/me",f.base)).header(header::COOKIE,cookie).send().await.unwrap();
        let user: Value = response.json().await.unwrap();
        assert_eq!(user["role"],role,"{mode}: {user}");
        assert_eq!(user["name"],"Test member");
        assert_eq!(f.provider.userinfo_reads.load(Ordering::SeqCst), usize::from(mode == "userinfo"));
        assert_eq!(f.provider.jwks_reads.load(Ordering::SeqCst), if mode == "rotation" { 2 } else { 1 });
    }
}

#[tokio::test]
async fn protects_all_pages_and_api_and_sessions_survive_restart() {
    let f = Fixture::new("normal").await;
    for path in ["/", "/plans", "/assets/missing.js"] {
        let response = f.client.get(format!("{}{path}",f.base)).send().await.unwrap();
        assert_eq!(response.status(),StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION],"/auth/login");
    }
    for path in ["/api", "/api/v1/snapshot", "/api/v1/live", "/api/unknown"] {
        assert_eq!(f.client.get(format!("{}{path}",f.base)).send().await.unwrap().status(),StatusCode::UNAUTHORIZED);
    }
    let favicon = f.client.get(format!("{}/favicon.svg",f.base)).header("sec-fetch-mode","no-cors").send().await.unwrap();
    assert_eq!(favicon.status(),StatusCode::UNAUTHORIZED,"a subresource must not start a sign-in");
    let page = f.client.get(format!("{}/plans",f.base)).header("sec-fetch-mode","navigate").send().await.unwrap();
    assert_eq!(page.headers()[header::LOCATION],"/auth/login");
    let cookie = f.session().await;
    let mut headers = HeaderMap::new();
    headers.insert(header::COOKIE,cookie.parse().unwrap());
    let restarted = Auth::connect(f.auth.cfg.clone(), Arc::new(History::open(&f.path.join("history.db")).unwrap()),"test-secret".into()).await.unwrap();
    assert_eq!(restarted.session(&headers).await.unwrap().unwrap().role,"admin");
    assert_eq!(f.provider.userinfo_reads.load(Ordering::SeqCst),0);
    let raw = cookie.split_once('=').unwrap().1;
    let db = rusqlite::Connection::open(f.path.join("history.db")).unwrap();
    let stored: String = db.query_row("SELECT token_hash FROM web_sessions",[],|r| r.get(0)).unwrap();
    assert_eq!(stored,hash(raw));
    assert_ne!(stored,raw);
}

#[tokio::test]
async fn logout_and_expiry_end_sessions_and_the_live_socket() {
    use futures::StreamExt;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    for expire in [false,true] {
        let f = Fixture::new("normal").await;
        let cookie = f.session().await;
        let live = |origin: &str| {
            let mut request = format!("{}/api/v1/live",f.base.replacen("http","ws",1)).into_client_request().unwrap();
            request.headers_mut().insert(header::COOKIE,cookie.parse().unwrap());
            request.headers_mut().insert(header::ORIGIN,origin.parse().unwrap());
            request
        };
        // Behind OIDC the browser's Origin is the configured public one, not this server's Host.
        let refused = tokio_tungstenite::connect_async(live("https://evil.example")).await.unwrap_err();
        assert!(matches!(refused, tokio_tungstenite::tungstenite::Error::Http(ref r) if r.status() == StatusCode::FORBIDDEN), "{refused:?}");
        let (mut socket, _) = tokio_tungstenite::connect_async(live("https://forkdeck.example")).await.unwrap();
        if expire {
            rusqlite::Connection::open(f.path.join("history.db")).unwrap().execute("UPDATE web_sessions SET expires = 0",[]).unwrap();
        } else {
            assert_eq!(f.client.post(format!("{}/auth/logout",f.base)).header(header::COOKIE,&cookie).header(header::ORIGIN,"https://evil.example").send().await.unwrap().status(),StatusCode::FORBIDDEN);
            let logout = f.client.post(format!("{}/auth/logout",f.base)).header(header::COOKIE,&cookie).header(header::ORIGIN,"https://forkdeck.example").send().await.unwrap();
            assert_eq!(logout.status(),StatusCode::OK);
            // Browsers send the page's Origin on the sign-out form only when the policy allows it.
            assert_eq!(logout.headers()["referrer-policy"],"same-origin");
            assert!(logout.headers()[header::SET_COOKIE].to_str().unwrap().contains("Max-Age=0"));
            assert!(logout.text().await.unwrap().contains("href=\"/auth/login\""));
        }
        assert_eq!(f.client.get(format!("{}/api/v1/me",f.base)).header(header::COOKIE,cookie).send().await.unwrap().status(),StatusCode::UNAUTHORIZED);
        tokio::time::timeout(Duration::from_secs(3),async {
            while let Some(Ok(message)) = socket.next().await { if message.is_close() { break; } }
        }).await.expect("the live socket must close promptly");
    }
}

#[test]
fn validates_configuration() {
    let base = json!({"issuer":"https://identity.example","client_id":"client","client_secret_env":"OIDC_SECRET","redirect_url":"https://forkdeck.example/auth/callback","roles":{"viewer":["readers"]}});
    let valid: OidcCfg = serde_json::from_value(base.clone()).unwrap();
    valid.validate().unwrap();
    assert_eq!(valid.scopes, ["openid","email","profile","groups"]);
    assert_eq!(valid.session_lifetime_secs,43200);
    for (key,value) in [("issuer",json!("http://public.example")),("redirect_url",json!("http://forkdeck.example/auth/callback")),("client_id",json!("")),("client_secret_env",json!("the secret value")),("groups_claim",json!("realm..roles")),("roles",json!({})),("scopes",json!(["profile"])),("session_lifetime_secs",json!(0))] {
        let mut bad = base.clone(); bad[key] = value;
        assert!(serde_json::from_value::<OidcCfg>(bad).unwrap().validate().is_err(),"{key}");
    }
}


#[tokio::test]
async fn a_new_signin_ends_the_previous_browser_session() {
    let f = Fixture::new("normal").await;
    let cookie = f.session().await;
    let response = f.client.get(format!("{}/auth/login",f.base)).header(header::COOKIE,&cookie).send().await.unwrap();
    assert_eq!(response.status(),StatusCode::SEE_OTHER);
    assert!(response.headers().get_all(header::SET_COOKIE).iter().any(|v| v.to_str().unwrap().starts_with("forkdeck_session=;")));
    assert_eq!(f.client.get(format!("{}/api/v1/me",f.base)).header(header::COOKIE,cookie).send().await.unwrap().status(),StatusCode::UNAUTHORIZED);
}

// Opt-in browser fixture: the capture script and browser tests supply a loopback HTTPS proxy for
// Secure cookies. FORKDECK_CAPTURE_ROLE=viewer signs in a member who may only read.
#[tokio::test]
#[ignore = "started by web/scripts/screenshots.mjs and web/e2e"]
async fn screenshot_provider() {
    let viewer = std::env::var("FORKDECK_CAPTURE_ROLE").as_deref() == Ok("viewer");
    let f = Fixture::new(if viewer { "capture-viewer" } else { "capture" }).await;
    println!("Forkdeck capture fixture: {}", f.base);
    tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn viewer_cannot_write_but_operator_and_admin_can() {
    for (mode, status) in [("userinfo",StatusCode::FORBIDDEN),("nested",StatusCode::OK),("rotation",StatusCode::OK)] {
        let fixture = Fixture::new(mode).await;
        let cookie = fixture.session().await;
        let response = fixture.client.post(format!("{}/api/v1/actions",fixture.base))
            .header(header::COOKIE,cookie).header(header::ORIGIN,"https://forkdeck.example")
            .header("idempotency-key","intake").json(&json!({"action":"automation","on":true})).send().await.unwrap();
        assert_eq!(response.status(),status,"{mode}");
    }
}

#[tokio::test]
async fn oidc_viewer_cannot_connect_or_add_a_repository() {
    let f = Fixture::new("userinfo").await;
    let cookie = f.session().await;
    let cfg = toml::from_str(r#"
[web.forges.github]
client_id = "fixture"
client_secret_env = "FORKDECK_FORGE_ROLE_TEST_SECRET"
redirect_url = "https://forkdeck.example/forges/github/callback"
"#).unwrap();
    let hub = crate::orchestrator::Hub::new(cfg,tokio::runtime::Handle::current());
    let app = crate::server::http::App { hub, feed:Arc::new(crate::remote::feed::Feed::new()),loopback:true,auth:Some(f.auth.clone()),writes:Default::default(),forges:Default::default() };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}",listener.local_addr().unwrap());
    let router = crate::server::http::router(app);
    let task = tokio::spawn(async move { axum::serve(listener,router).await.unwrap(); });
    for path in ["/forges/github/connect","/forges/github/callback?code=x&state=x"] {
        assert_eq!(f.client.get(format!("{base}{path}")).header(header::COOKIE,&cookie).send().await.unwrap().status(),StatusCode::FORBIDDEN);
    }
    let body: Value = f.client.get(format!("{base}/api/v1/forges")).header(header::COOKIE,&cookie).send().await.unwrap().json().await.unwrap();
    assert_eq!(body["can_add"],false);
    let response = f.client.post(format!("{base}/api/v1/actions")).header(header::COOKIE,&cookie)
        .header(header::ORIGIN,"https://forkdeck.example").header("idempotency-key","viewer-add")
        .json(&json!({"action":"add_repo","picker":"missing","repo":"owner/repo"})).send().await.unwrap();
    assert_eq!(response.status(),StatusCode::FORBIDDEN);
    task.abort();
}
