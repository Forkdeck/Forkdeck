//! Optional OIDC boundary. No provider tokens or raw session tokens enter durable storage.
use crate::{config::OidcCfg, history::History};
use axum::{extract::{Query, State}, http::{header, HeaderMap, StatusCode}, response::{Html, IntoResponse, Redirect, Response}, routing::get, Json, Router};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use openidconnect::{core::{CoreClient, CoreProviderMetadata, CoreGenderClaim, CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm}, AccessTokenHash, AdditionalClaims, AuthenticationFlow, AuthorizationCode, ClaimsVerificationError, ClientId, ClientSecret, CsrfToken, IdToken, IssuerUrl, Nonce, OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, SignatureVerificationError, TokenResponse, UserInfoClaims};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Arc, time::{Duration, Instant}};
use tokio::sync::Mutex;

const FLOW: &str = "forkdeck_oidc";
const SESSION: &str = "forkdeck_session";
const FLOW_SECS: i64 = 300;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Principal {
    pub subject: String,
    pub name: String,
    pub groups: Vec<String>,
    pub role: String,
}

pub(super) fn can_write(principal: Option<&Principal>) -> bool {
    principal.is_none_or(|p| matches!(p.role.as_str(), "operator" | "admin"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Extra { #[serde(flatten)] claims: HashMap<String, serde_json::Value> }
impl AdditionalClaims for Extra {}
type Token = IdToken<Extra, CoreGenderClaim, CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm>;

#[derive(Serialize, Deserialize)]
struct Flow { state: String, nonce: String, verifier: String, expires: i64 }

pub struct Auth {
    cfg: OidcCfg,
    secret: String,
    http: reqwest::Client,
    metadata: Mutex<(CoreProviderMetadata, Instant)>,
    key: [u8; 32],
    pending: Mutex<HashMap<String, i64>>,
    history: Arc<History>,
    scope: String,
}

fn now() -> i64 { chrono::Utc::now().timestamp() }
fn random() -> String {
    let mut bytes = [0; 32];
    getrandom::getrandom(&mut bytes).expect("OS randomness");
    URL_SAFE_NO_PAD.encode(bytes)
}
fn hash(token: &str) -> String { URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes())) }
pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get_all(header::COOKIE).iter().filter_map(|h| h.to_str().ok()).flat_map(|h| h.split(';')).find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key == name).then(|| value.to_owned())
    })
}
fn set_cookie(name: &str, value: &str, seconds: i64, secure: bool) -> String {
    format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={seconds}{}", if secure { "; Secure" } else { "" })
}
fn with_cookie(mut response: Response, value: String) -> Response {
    response.headers_mut().append(header::SET_COOKIE, value.parse().unwrap());
    response.headers_mut().insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

impl Auth {
    pub(super) fn origin(&self) -> String {
        reqwest::Url::parse(&self.cfg.redirect_url).unwrap().origin().ascii_serialization()
    }

    pub async fn new(cfg: OidcCfg, history: Arc<History>) -> anyhow::Result<Arc<Self>> {
        cfg.validate()?;
        let secret = std::env::var(&cfg.client_secret_env).ok().filter(|s| !s.trim().is_empty()).ok_or_else(|| anyhow::anyhow!("web.oidc.client_secret_env must refer to a nonempty environment variable"))?;
        Self::connect(cfg, history, secret).await
    }

    async fn connect(cfg: OidcCfg, history: Arc<History>, secret: String) -> anyhow::Result<Arc<Self>> {
        let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(15)).build()?;
        let metadata = CoreProviderMetadata::discover_async(IssuerUrl::new(cfg.issuer.clone())?, &http).await?;
        anyhow::ensure!(metadata.token_endpoint().is_some(), "OIDC discovery has no token endpoint");
        let mut key = [0; 32];
        getrandom::getrandom(&mut key).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let scope = hash(&serde_json::to_string(&cfg)?);
        Ok(Arc::new(Self { cfg, secret, http, metadata: Mutex::new((metadata, Instant::now())), key, pending: Mutex::new(HashMap::new()), history, scope }))
    }

    async fn metadata(&self, refresh: bool) -> anyhow::Result<CoreProviderMetadata> {
        let mut cached = self.metadata.lock().await;
        if refresh || cached.1.elapsed() > Duration::from_secs(3600) {
            let fresh = CoreProviderMetadata::discover_async(IssuerUrl::new(self.cfg.issuer.clone())?, &self.http).await?;
            *cached = (fresh, Instant::now());
        }
        Ok(cached.0.clone())
    }

    fn sign(&self, flow: &Flow) -> String {
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(flow).unwrap());
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).unwrap();
        mac.update(payload.as_bytes());
        format!("{payload}.{}", URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
    }

    fn flow(&self, headers: &HeaderMap) -> anyhow::Result<Flow> {
        let value = cookie(headers, FLOW).ok_or_else(|| anyhow::anyhow!("missing flow"))?;
        let (payload, signature) = value.split_once('.').ok_or_else(|| anyhow::anyhow!("invalid flow"))?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).unwrap();
        mac.update(payload.as_bytes());
        mac.verify_slice(&URL_SAFE_NO_PAD.decode(signature)?).map_err(|_| anyhow::anyhow!("invalid flow signature"))?;
        let flow: Flow = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload)?)?;
        anyhow::ensure!(flow.expires > now(), "flow expired");
        Ok(flow)
    }

    pub async fn session(&self, headers: &HeaderMap) -> anyhow::Result<Option<Principal>> {
        let Some(token) = cookie(headers, SESSION) else { return Ok(None) };
        self.session_hash(hash(&token)).await
    }

    pub async fn session_hash(&self, hash: String) -> anyhow::Result<Option<Principal>> {
        let history = self.history.clone();
        let scope = self.scope.clone();
        let json = tokio::task::spawn_blocking(move || history.web_session(&hash, &scope)).await?.map_err(anyhow::Error::msg)?;
        json.map(|s| serde_json::from_str(&s).map_err(anyhow::Error::from)).transpose()
    }

    pub fn session_key(&self, headers: &HeaderMap) -> Option<String> { cookie(headers, SESSION).map(|token| hash(&token)) }

    async fn revoke(&self, headers: &HeaderMap) -> anyhow::Result<()> {
        if let Some(key) = self.session_key(headers) {
            let history = self.history.clone();
            tokio::task::spawn_blocking(move || history.web_session_delete(&key)).await?.map_err(anyhow::Error::msg)?;
        }
        Ok(())
    }

    async fn complete(&self, query: Callback, headers: &HeaderMap) -> anyhow::Result<String> {
        let flow = self.flow(headers)?;
        // Compare fixed-size hashes in constant time, then consume the flow even on a failed exchange.
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).unwrap();
        mac.update(flow.state.as_bytes());
        let expected = mac.finalize().into_bytes();
        let mut actual = Hmac::<Sha256>::new_from_slice(&self.key).unwrap();
        actual.update(query.state.as_bytes());
        actual.verify_slice(&expected).map_err(|_| anyhow::anyhow!("state mismatch"))?;
        anyhow::ensure!(self.pending.lock().await.remove(&hash(&flow.state)).is_some_and(|expiry| expiry > now()), "flow already used or expired");
        let metadata = self.metadata(false).await?;
        let client = CoreClient::from_provider_metadata(metadata, ClientId::new(self.cfg.client_id.clone()), Some(ClientSecret::new(self.secret.clone())))
            .set_redirect_uri(RedirectUrl::new(self.cfg.redirect_url.clone())?);
        let response = client.exchange_code(AuthorizationCode::new(query.code))?.set_pkce_verifier(PkceCodeVerifier::new(flow.verifier)).request_async(&self.http).await?;
        let token: Token = response.id_token().ok_or_else(|| anyhow::anyhow!("missing ID token"))?.to_string().parse()?;
        anyhow::ensure!(!matches!(token.signing_alg()?, CoreJwsSigningAlgorithm::HmacSha256 | CoreJwsSigningAlgorithm::HmacSha384 | CoreJwsSigningAlgorithm::HmacSha512), "ID tokens must use an asymmetric JWKS signature");
        let nonce = Nonce::new(flow.nonce);
        let verifier = client.id_token_verifier();
        let mut verified = token.claims(&verifier, &nonce).cloned();
        if matches!(verified, Err(ClaimsVerificationError::SignatureVerification(SignatureVerificationError::NoMatchingKey))) {
            let refreshed = CoreClient::from_provider_metadata(self.metadata(true).await?, ClientId::new(self.cfg.client_id.clone()), Some(ClientSecret::new(self.secret.clone())));
            verified = token.claims(&refreshed.id_token_verifier(), &nonce).cloned();
        }
        let claims = verified?;
        if let Some(expected) = claims.access_token_hash() {
            let current = CoreClient::from_provider_metadata(self.metadata(false).await?, ClientId::new(self.cfg.client_id.clone()), Some(ClientSecret::new(self.secret.clone())));
            let verifier = current.id_token_verifier();
            let actual = AccessTokenHash::from_token(response.access_token(), token.signing_alg()?, token.signing_key(&verifier)?)?;
            anyhow::ensure!(&actual == expected, "access token hash mismatch");
        }
        let value = serde_json::to_value(&claims)?;
        let groups = match self.groups(&value)? {
            Some(groups) => groups,
            None => {
                let info: UserInfoClaims<Extra, CoreGenderClaim> = client.user_info(response.access_token().clone(), Some(claims.subject().clone()))?.request_async(&self.http).await?;
                self.groups(&serde_json::to_value(info)?)?.ok_or_else(|| anyhow::anyhow!("missing groups"))?
            }
        };
        let role = [(&self.cfg.roles.admin, "admin"), (&self.cfg.roles.operator, "operator"), (&self.cfg.roles.viewer, "viewer")].into_iter()
            .find(|(allowed, _)| groups.iter().any(|group| allowed.contains(group))).map(|(_, role)| role).ok_or_else(|| anyhow::anyhow!("no allowed group"))?;
        let principal = Principal { subject: claims.subject().as_str().into(), name: claims.name().and_then(|n| n.get(None)).map(|n| n.as_str()).or_else(|| claims.email().map(|e| e.as_str())).unwrap_or(claims.subject().as_str()).into(), groups, role: role.into() };
        let token = random();
        let token_hash = hash(&token);
        let json = serde_json::to_string(&principal)?;
        let history = self.history.clone();
        let scope = self.scope.clone();
        let expires = now() + self.cfg.session_lifetime_secs as i64;
        tokio::task::spawn_blocking(move || history.web_session_save(&token_hash, &scope, &json, expires)).await?.map_err(anyhow::Error::msg)?;
        Ok(token)
    }

    fn groups(&self, claims: &serde_json::Value) -> anyhow::Result<Option<Vec<String>>> {
        let mut value = claims;
        for part in self.cfg.groups_claim.split('.') {
            let Some(next) = value.get(part) else { return Ok(None) };
            value = next;
        }
        let groups = value.as_array().ok_or_else(|| anyhow::anyhow!("groups claim must be an array"))?;
        Ok(Some(groups.iter().map(|g| g.as_str().map(String::from).ok_or_else(|| anyhow::anyhow!("group must be a string"))).collect::<anyhow::Result<_>>()?))
    }
}

#[derive(Deserialize)]
struct Callback { code: String, state: String }

async fn login(State(auth): State<Arc<Auth>>, headers: HeaderMap) -> Response {
    let result: anyhow::Result<_> = async {
        auth.revoke(&headers).await?;
        let client = CoreClient::from_provider_metadata(auth.metadata(false).await?, ClientId::new(auth.cfg.client_id.clone()), Some(ClientSecret::new(auth.secret.clone())))
            .set_redirect_uri(RedirectUrl::new(auth.cfg.redirect_url.clone())?);
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let mut request = client.authorize_url(AuthenticationFlow::<openidconnect::core::CoreResponseType>::AuthorizationCode, CsrfToken::new_random, Nonce::new_random).set_pkce_challenge(challenge);
        for scope in &auth.cfg.scopes { if scope != "openid" { request = request.add_scope(Scope::new(scope.clone())); } }
        let (url, state, nonce) = request.url();
        let flow = Flow { state: state.secret().clone(), nonce: nonce.secret().clone(), verifier: verifier.secret().clone(), expires: now() + FLOW_SECS };
        let mut pending = auth.pending.lock().await;
        pending.retain(|_, expiry| *expiry > now());
        anyhow::ensure!(pending.len() < 4096, "too many pending sign-ins");
        pending.insert(hash(&flow.state), flow.expires);
        let response = with_cookie(Redirect::to(url.as_str()).into_response(), set_cookie(FLOW, &auth.sign(&flow), FLOW_SECS, true));
        Ok(with_cookie(response, set_cookie(SESSION, "", 0, auth.cfg.redirect_url.starts_with("https:"))))
    }.await;
    result.unwrap_or_else(|_| (StatusCode::SERVICE_UNAVAILABLE, "Sign-in unavailable").into_response())
}

async fn callback(State(auth): State<Arc<Auth>>, Query(query): Query<Callback>, headers: HeaderMap) -> Response {
    let response = match auth.complete(query, &headers).await {
        Ok(token) => with_cookie(Redirect::to("/").into_response(), set_cookie(SESSION, &token, auth.cfg.session_lifetime_secs as i64, auth.cfg.redirect_url.starts_with("https:"))),
        Err(_) => (StatusCode::FORBIDDEN, "Sign-in failed or no permitted group. Start again at /auth/login.").into_response(),
    };
    with_cookie(response, set_cookie(FLOW, "", 0, true))
}

async fn logout(State(auth): State<Arc<Auth>>, headers: HeaderMap) -> Response {
    let origin = reqwest::Url::parse(&auth.cfg.redirect_url).unwrap().origin().ascii_serialization();
    if headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) != Some(origin.as_str()) { return StatusCode::FORBIDDEN.into_response(); }
    if auth.revoke(&headers).await.is_err() { return StatusCode::SERVICE_UNAVAILABLE.into_response(); }
    with_cookie(signed_out().await.into_response(), set_cookie(SESSION, "", 0, auth.cfg.redirect_url.starts_with("https:")))
}

async fn signed_out() -> Html<&'static str> {
    Html(r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Forkdeck · Sign in</title><body style="margin:0;background:#0F1115;color:#E6E8EC;font:14px 'IBM Plex Sans',system-ui,sans-serif"><main style="margin:64px auto;padding:24px;max-width:420px;background:#171A21;border:1px solid #2A2F3A;border-radius:10px"><h1 style="font-size:20px">forkdeck</h1><p>Sign in to access this workspace.</p><a style="color:#F2A541" href="/auth/login">Sign in</a></main></body></html>"#)
}

pub fn routes<S: Clone + Send + Sync + 'static>(auth: Arc<Auth>) -> Router<S> {
    Router::new().route("/auth/login", get(login)).route("/auth/callback", get(callback)).route("/auth/logout", get(signed_out).post(logout)).with_state(auth)
}

pub async fn whoami(principal: Option<axum::Extension<Principal>>) -> Json<Option<Principal>> { Json(principal.map(|p| p.0)) }

#[cfg(test)]
mod tests;
