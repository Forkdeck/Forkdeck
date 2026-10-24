import { useEffect, useState } from "preact/hooks";
import { Account } from "./Account";
import { Navigation } from "./Operational";
import { action, actionBusy, actionError, readForges, writeReason } from "./state";
import type { AddedRepo, Forges, RepoPickerData } from "./state";

function useForges() {
  const [forges, setForges] = useState<Forges>();
  const [error, setError] = useState("");
  useEffect(() => {
    let alive = true;
    void readForges().then(value => { if (alive) setForges(value); })
      .catch(e => { if (alive) setError((e as Error).message); });
    return () => { alive = false; };
  }, []);
  return { forges, error };
}

function ForgeLinks({ forges }: { forges?: Forges }) {
  if (!forges?.providers.length) return null;
  const reason = !forges.can_add ? "Operator or admin role required" : writeReason.value;
  return <div class="forge-connect">
    <div class="forge-links">{forges.providers.map(provider => {
      const label = `Add from ${provider.id === "github" ? "GitHub" : "GitLab"}`;
      return reason
        ? <button key={provider.id} class="secondary-button" type="button" disabled title={reason}>{label}</button>
        : <a key={provider.id} class="issue-link" href={`/forges/${provider.id}/connect`} onClick={e => e.stopPropagation()}>{label}</a>;
    })}</div>
    {reason && <p class="action-notice">{reason}</p>}
  </div>;
}

export function AddRepositoryLinks() {
  const { forges, error } = useForges();
  return <>{error && <p role="alert">{error}</p>}<ForgeLinks forges={forges} /></>;
}

export function RepoPicker({ id }: { id?: string }) {
  const { forges, error: forgeError } = useForges();
  const [picker, setPicker] = useState<RepoPickerData>();
  const [search, setSearch] = useState("");
  const [error, setError] = useState("");
  const [expired, setExpired] = useState(false);
  const [added, setAdded] = useState<AddedRepo[]>([]);
  const [adding, setAdding] = useState("");
  useEffect(() => {
    let alive = true;
    setPicker(undefined);
    setExpired(false);
    setError("");
    setSearch("");
    setAdded([]);
    const load = async () => {
      try {
        const response = await fetch(`/api/v1/forges/pickers/${encodeURIComponent(id ?? "")}`);
        if (response.status === 401) { location.assign("/auth/login"); return; }
        if (response.status === 404) { if (alive) setExpired(true); return; }
        if (!response.ok) {
          const result = await response.json().catch(() => ({}));
          throw new Error(result.error ?? `Could not load repository list (${response.status})`);
        }
        const result: RepoPickerData = await response.json();
        if (alive) setPicker(result);
      } catch (e) { if (alive) setError((e as Error).message); }
    };
    void load();
    return () => { alive = false; };
  }, [id]);
  useEffect(() => {
    if (!picker) return;
    const timer = setTimeout(() => setExpired(true), Math.max(0, picker.expires_unix * 1000 - Date.now()));
    return () => clearTimeout(timer);
  }, [picker]);
  const reason = forges?.can_add === false ? "Operator or admin role required" : writeReason.value;
  const disabled = !forges?.can_add || Boolean(reason) || actionBusy.value;
  const add = async (repo: string) => {
    if (disabled || expired) return;
    setAdding(repo);
    setError("");
    const result = await action<AddedRepo>({ action: "add_repo", picker: id, repo });
    if (result) setAdded(previous => [...previous, result]);
    else setError(actionError.value);
    setAdding("");
  };
  const query = search.trim().toLocaleLowerCase();
  const repos = picker?.repos.filter(repo => `${repo.repo}\n${repo.description ?? ""}`.toLocaleLowerCase().includes(query)) ?? [];
  return <div class="app-shell">
    <header class="app-header">
      <a class="brand-link plan-brand" href="/">forkdeck</a>
      <span class="muted-label">Repositories</span>
      <span class="grow" />
      <Account />
    </header>
    <div class="app-body">
      <aside class="operations-sidebar"><Navigation /></aside>
      <main class="operations-main repo-picker">
        <h1>{picker ? `Add a repository from ${picker.host}` : "Add a repository"}</h1>
        {added.map(result => <p key={result.repo} class="operation-panel" role="status">
          Added <strong>{result.repo}</strong> at <code>{result.path}</code>. Restart Forkdeck to start watching it.
        </p>)}
        {(error || forgeError) && <p role="alert">{error || forgeError}</p>}
        {expired ? <>
          <p>This list expired. Connect again.</p>
          <ForgeLinks forges={forges} />
        </> : picker ? <>
          {reason && <p class="action-notice">{reason}</p>}
          <label class="repo-search">Search repositories
            <input class="reply-input" type="search" value={search} onInput={e => setSearch(e.currentTarget.value)} />
          </label>
          {repos.length ? <ul class="repo-picker-list">{repos.map(repo => {
            const configured = repo.configured || added.some(result => result.repo === repo.repo);
            return <li key={repo.repo} class="operation-panel repo-picker-row">
              <div class="repo-picker-copy">
                <h2>{repo.repo}</h2>
                <div class="repo-picker-meta">
                  <span class="task-pill tone-group">{repo.private ? "Private" : "Public"}</span>
                  <span class="mono-muted">Default branch: <span>{repo.default_branch}</span></span>
                </div>
                {repo.description && <p>{repo.description}</p>}
              </div>
              {configured ? <span class="task-pill tone-merged">Already added</span>
                : <button class="primary-button" type="button" disabled={disabled} title={reason} onClick={() => void add(repo.repo)}>{adding === repo.repo ? "Adding…" : "Add"}</button>}
            </li>;
          })}</ul> : <p>{picker.repos.length ? "No repositories match your search." : "No repositories available from this account."}</p>}
        </> : !error && !expired && <p role="status">Loading…</p>}
      </main>
    </div>
  </div>;
}
