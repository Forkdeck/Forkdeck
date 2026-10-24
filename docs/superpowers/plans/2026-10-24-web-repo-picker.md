# Repository Picker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** From the web UI, a person connects their GitHub or GitLab account with OAuth, picks one of their repositories, and Forkdeck clones it and adds its `[[repos]]` entry.

**Architecture:** Each forge is an optional `[web.forges.<provider>]` OAuth app. `GET /forges/{provider}/connect` starts an authorization-code flow; `GET /forges/{provider}/callback` exchanges the code, lists the person's repositories, **discards the token**, and keeps only the list in memory as a short-lived *picker* bound to that browser. The browser shows the picker and adds a repository through the existing guarded `POST /api/v1/actions` (`add_repo`); the server clones it with the daemon's own `gh`/`glab` login and appends a `[[repos]]` block to the config file. Watching it starts after a daemon restart.

**Tech Stack:** Rust (axum 0.8, reqwest, existing `server::auth` and `server::write`), Preact + TypeScript.

**Spec:** this conversation's decisions: picker only (no forge sign-in); per-user OAuth whose token lists repositories and is never stored; issue/PR work stays on the daemon's `gh`/`glab`. Builds on `feat/web-ui` at `4b2a83a` (optional OIDC in `src/server/auth.rs`, `docs/web-ui-oidc.md`; guarded actions in `src/server/write.rs`).

## Global Constraints

- No provider token is written anywhere: not to SQLite, files, logs, audit records, responses, or error messages. It lives only inside the callback handler.
- Secrets come from environment variables named in config (`client_secret_env`), like `[web.oidc]`. Missing/empty secret, invalid URLs, or a non-HTTPS endpoint other than loopback fail startup.
- Provider HTTP requests: 15-second timeout, no redirects followed (same as OIDC discovery).
- Without any `[web.forges.*]` the feature is invisible: no routes answer (404), no UI controls.
- Adding a repository is a browser write: it goes through `POST /api/v1/actions` and inherits its Origin check, `Idempotency-Key`, size limit, role check (operator/admin under OIDC; viewer 403) and audit record. Connect and callback require the same role.
- `--demo` refuses `[web.forges.*]` at startup (no network, no clones), like OIDC.
- Request bodies carry only the picker id and a repository name from that picker, never a URL, path, host or provider (the existing write rule: "never execution configuration").
- Hand-formatted Rust: never run `cargo fmt`/`rustfmt`. Real data only in `web/`. Do not change the Content-Security-Policy.
- Gate before each commit, unpiped, each must exit 0: `cargo clippy --all-targets -- -D warnings`, `cargo test`, and in `web/`: `npm run check`, `npm run build` (commit the rebuilt `web/dist`).

## Review Focus

1. **Login CSRF / mixed-up callback.** A callback whose `state` is unknown, expired (5 min), already used, or not bound to this browser's cookie must be refused (400) without contacting the provider. Test in Task 2.
2. **Token leak through errors.** A provider that answers the repo listing with an error body echoing the token must not put that body into the response, log or audit. Test in Task 2.
3. **A repository the daemon cannot clone.** The person can see it but the daemon's `gh`/`glab` login cannot clone it: the action fails with the CLI's message, nothing is appended to the config, and no half-clone is left. Test in Task 3.
4. **Adding twice / already configured.** A repository already in `[[repos]]` (same provider, host and name) or already cloned at the target path is refused (409) and the config is unchanged. Test in Task 3.
5. **Concurrent edits to the config file.** Two adds at once, or a config file edited by hand meanwhile: appends must be serialized and must never rewrite or reorder existing content; if the file no longer parses after the append it is restored. Test in Task 3.

---

## File Structure

- `src/config.rs` — `ForgeAppCfg`, `WebCfg.forges`, validation.
- `src/server/forges.rs` (new) — OAuth connect/callback, provider clients, the in-memory picker store, `GET /api/v1/forges`, `GET /api/v1/forges/pickers/{id}`.
- `src/server/write.rs` — `Command::AddRepo` and its domain function.
- `src/server/http.rs` — routes.
- `web/src/RepoPicker.tsx` (new), `web/src/Operational.tsx`, `web/src/main.tsx`, `web/src/state.ts` — UI.
- `docs/web-ui-forges.md` (new), `forkdeck.example.toml`.

### Task 1: Configuration

**Files:** Modify `src/config.rs`, `forkdeck.example.toml`; create `docs/web-ui-forges.md`.

**Interfaces:**
- Produces:

```rust
pub struct WebCfg { /* existing */ pub forges: ForgeApps }
#[serde(default, deny_unknown_fields)]
pub struct ForgeApps { pub github: Option<ForgeAppCfg>, pub gitlab: Option<ForgeAppCfg> }
#[serde(deny_unknown_fields)]
pub struct ForgeAppCfg {
    /// "github.com" / "gitlab.com" by default; a GitHub Enterprise or self-managed GitLab host otherwise.
    #[serde(default)] pub host: String,
    pub client_id: String,
    pub client_secret_env: String,
    /// Public URL of `/forges/<provider>/callback`; HTTPS except on loopback.
    pub redirect_url: String,
    /// GitHub only: also list private repositories (needs the `repo` scope).
    #[serde(default)] pub private: bool,
    /// Where picked repositories are cloned: <clone_dir>/<host>/<owner path>. Default ~/.local/share/forkdeck/repos.
    #[serde(default)] pub clone_dir: PathBuf,
    /// Tests only: API and OAuth base URL override (loopback http allowed).
    #[serde(default)] pub base_url: String,
}
impl ForgeAppCfg { pub fn secret(&self) -> anyhow::Result<String>; }
```

  Endpoints derived from `host` unless `base_url` is set: GitHub `https://<host>/login/oauth/authorize`, `/login/oauth/access_token`, API `https://api.github.com` (`https://<host>/api/v3` for Enterprise); GitLab `https://<host>/oauth/authorize`, `/oauth/token`, API `https://<host>/api/v4`. Scopes: GitHub none (public only) or `repo` when `private = true`; GitLab `read_api`.

- [ ] **Step 1: Failing tests** in `config.rs`: a full `[web.forges.github]`/`[web.forges.gitlab]` example parses with defaults applied; startup validation fails, naming the key, for: empty `client_id`, unset/empty secret env, non-HTTPS `redirect_url` on a non-loopback host, `redirect_url` path not `/forges/<provider>/callback`, `--demo` with any forge configured; the shipped `forkdeck.example.toml` (forge block commented out) still parses.
- [ ] **Step 2:** Run `cargo test config::` — fails.
- [ ] **Step 3:** Implement; add the commented example block and `docs/web-ui-forges.md` (registering the OAuth app on GitHub and GitLab, scopes and why, callback URL, the token-not-stored rule, the restart after adding).
- [ ] **Step 4:** Tests pass. Gate. Commit `feat(config): optional GitHub/GitLab OAuth apps for the web UI`.

### Task 2: Connect, callback and picker

**Files:** Create `src/server/forges.rs`; modify `src/server/http.rs`, `src/server/mod.rs`.

**Interfaces:**
- Consumes: `ForgeAppCfg`; `server::auth` role check for the current request (the same helper `write::action` uses); `App`.
- Produces:
  - `GET /forges/{provider}/connect` → 303 to the provider's authorize URL with `client_id`, `redirect_uri`, scopes, `state` (random 256-bit, URL-safe) and PKCE S256 `code_challenge`; sets cookie `forkdeck_forge` (random 256-bit, HttpOnly, SameSite=Lax, Path=/forges, Secure unless the redirect URL is loopback http, Max-Age=300). Server keeps `{state → (cookie hash, provider, verifier, expires)}` in memory, max 64 pending, oldest dropped.
  - `GET /forges/{provider}/callback?code&state` → validates state (exists, provider matches, not expired, cookie matches, single use), exchanges the code (with `code_verifier`), lists repositories (GitHub `GET /user/repos?affiliation=owner,collaborator,organization_member&per_page=100`, GitLab `GET /projects?membership=true&simple=true&per_page=100`, following pagination up to 1000 repositories), drops the token, stores a picker `{id (random 256-bit), cookie hash, provider, host, repos, expires (10 min)}` and 303s to `/repos/add/<id>`. On a provider error: 502 page with a fixed message naming the step (exchange or listing) and the HTTP status only.
  - `GET /api/v1/forges` → `{"providers":[{"id":"github","host":"github.com"}],"can_add":bool}` (`can_add` false for viewers).
  - `GET /api/v1/forges/pickers/{id}` (must present the binding cookie) → `{"provider","host","expires_unix","repos":[{"repo":"owner/name","private":bool,"default_branch":"main","description":"…","configured":bool}]}`; 404 for unknown/expired/other browser.
  - `pub(super) fn picked(app: &App, picker: &str, cookie: &str, repo: &str) -> Option<PickedRepo>` with `pub(super) struct PickedRepo { provider: Provider, host: String, repo: String, default_branch: String }` for Task 3.

- [ ] **Step 1: Failing tests** against a fake provider (an axum server on 127.0.0.1 used via `base_url`): full happy path for each provider (connect → follow redirect to the fake authorize → callback → picker JSON lists the fake repos across two pages, `configured` true for one already in `[[repos]]`); Review Focus 1 (unknown, expired, reused, wrong-cookie, wrong-provider state → 400 and the fake saw no token request); Review Focus 2 (the fake listing returns 500 with the token in its body → response, captured logs and audit contain no token string); picker fetched with another cookie → 404; viewer under OIDC → 403 on connect; no `[web.forges]` → connect, callback and both API routes 404.
- [ ] **Step 2:** Run — fails.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Tests pass. Gate. Commit `feat(server): connect a forge account and list its repositories`.

### Task 3: Add a repository

**Files:** Modify `src/server/write.rs`, `src/config.rs` (append helper).

**Interfaces:**
- Consumes: `forges::picked`.
- Produces: `Command::AddRepo { picker: String, repo: String }` (action `"add_repo"`; the picker cookie comes from the request, not the body) → 200 `{"repo":"owner/name","path":"<clone path>","restart_required":true}`. Steps: look up `picked` (404 if absent); refuse 409 if configured or the clone path exists; `gh repo clone <repo> <path>` (GitHub; `--hostname` handling for Enterprise via `GH_HOST`) or `glab repo clone <repo> <path>` (GitLab, `GITLAB_HOST` for self-managed) into a temp sibling directory, then rename into place; append to the config file at `hub.config_path`:

```toml

[[repos]]
provider = "github"
repo = "owner/name"
path = "/abs/clone/path"
base = "main"
```

  under a process-wide mutex; re-parse the whole file with `Config` loading; on failure restore the previous bytes and remove the clone. `pub fn append_repo(path: &Path, entry: &RepoEntry) -> anyhow::Result<()>` in `config.rs`. No `config_path` (started with defaults only) → 409 "Forkdeck was started without a config file".

- [ ] **Step 1: Failing tests** using fake `gh`/`glab` scripts on PATH (see `tests/fixtures/fake-forge` and existing rigs): success appends exactly the block above, keeps prior bytes identical, clones to `<clone_dir>/<host>/<owner>/<name>`; Review Focus 3 (fake CLI exits 1 with "Could not resolve to a Repository" → 502 with that message, config bytes unchanged, no directory left); Review Focus 4 (already configured → 409; path exists → 409); Review Focus 5 (two concurrent adds of different repos → both blocks present, each once, file parses; a hand-made invalid edit appended between → restore and error); repo not in the picker → 404; body with an extra `path`/`url` field → 400 (deny_unknown_fields); viewer → 403; cross-origin → 403.
- [ ] **Step 2:** Run — fails.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Tests pass. Gate. Commit `feat(server): add a picked repository to the configuration`.

### Task 4: Picker UI

**Files:** Create `web/src/RepoPicker.tsx`; modify `web/src/Operational.tsx` (Repositories page), `web/src/main.tsx` (route `/repos/add/:id`), `web/src/state.ts`, `web/src/style.css`, `tests/web_pages.mjs`.

**Interfaces:**
- Consumes: Task 2's `GET /api/v1/forges` and `GET /api/v1/forges/pickers/{id}`; Task 3's `add_repo` action through the existing action client in `state.ts`/`Actions.tsx` (Idempotency-Key etc.).

- [ ] **Step 1:** Repositories page: when `providers` is non-empty, an "Add from GitHub" / "Add from GitLab" link per provider (`/forges/<id>/connect`, a plain navigation, not fetch); disabled with the required-role explanation when `can_add` is false (same pattern as other actions); absent when no providers.
- [ ] **Step 2:** `/repos/add/:id`: header "Add a repository from <host>", a search box filtering by name and description, a list (name, private/public pill, default branch, description, "Already added" for `configured`), an Add button per row; on success a confirmation naming the clone path and "Restart Forkdeck to start watching it."; on error the server's message; when the picker expired, "This list expired. Connect again." with the connect link. Phone layout below 700 px; follows the existing visual language.
- [ ] **Step 3:** `tests/web_pages.mjs`: in `--demo` (no forges), the Repositories page shows no add links and `/api/v1/forges` is 404; existing checks pass at 1440 and 390.
- [ ] **Step 4:** Gate plus `node tests/web_markdown.mjs` and the browser check against a demo server. Commit `feat(web): pick a repository from GitHub or GitLab`.
