import { signal, batch, computed } from "@preact/signals";
import type {
  BoardProjection,
  JobSummary,
  JobDetail as NativeJobDetail,
} from "./contracts";

export interface BoardJob extends JobSummary, BoardProjection {}
export interface JobDetail extends NativeJobDetail {
  board: BoardJob;
  revision: string;
  choices: { label: string; detail: string }[];
  queued: string[];
  actions: { reply: boolean; stop: boolean; retry: boolean; resume: boolean };
  /** Why this task takes no replies, when it does not. */
  reply_reason: string;
}
export interface PlanView {
  id: number | string;
  revision?: string;
  actions?: { publish: string | null; ready: string | null; refine: string | null };
  title: string;
  subtitle?: string;
  status: string;
  scheduler: string;
  description: string;
  draft_display: string;
  decision?: { number: number; title: string };
  plan: {
    repo: string;
    revision: number;
    reviewed?: number | null;
    accepted?: number | null;
    published?: number | null;
    issue?: { key: string; url: string };
    provider: string;
    draft: string;
    review: string;
    graph?: { tasks: { id: string; number?: number }[] };
  };
  tasks: {
    task: {
      id: string;
      number?: number;
      title: string;
      kind: string;
      parent?: string;
    };
    after: string[];
    status: string;
  }[];
}
export const principal = signal<{
  subject: string;
  name: string;
  groups: string[];
  role: string;
} | null>(null);
const identityLoaded = signal(false);
export const actionBusy = signal(false);
export const actionError = signal("");
/** Which control's action failed, so only that control shows the error. */
export const actionSource = signal("");
export const invalidation = signal(0);
export const writeReason = computed(() => !identityLoaded.value ? "Checking permissions…" : principal.value?.role === "viewer" ? "Operator or admin role required" : "");
export async function action<T = { id?: number }>(command: Record<string, unknown>, source = ""): Promise<T | undefined> {
  if (actionBusy.value || writeReason.value) return;
  actionBusy.value = true;
  actionError.value = "";
  actionSource.value = source;
  const key = Array.from(crypto.getRandomValues(new Uint8Array(16)), b => b.toString(16).padStart(2, "0")).join("");
  const options = { method: "POST", headers: { "Content-Type": "application/json", "Idempotency-Key": key }, body: JSON.stringify(command) };
  try {
    const response = await fetch("/api/v1/actions", options).catch(() => fetch("/api/v1/actions", options));
    if (response.status === 401) location.assign("/auth/login");
    const result = await response.json();
    if (!response.ok) throw new Error(result.error ?? `Action failed (${response.status})`);
    return result;
  } catch (e) { actionError.value = (e as Error).message; }
  finally { actionBusy.value = false; }
}
export interface Forges {
  providers: { id: "github" | "gitlab"; host: string }[];
  can_add: boolean;
}
export interface RepoPickerData {
  provider: "github" | "gitlab";
  host: string;
  expires_unix: number;
  repos: {
    repo: string;
    private: boolean;
    default_branch: string;
    description: string;
    configured: boolean;
  }[];
}
export interface AddedRepo {
  repo: string;
  path: string;
  restart_required: boolean;
}
export async function readForges(): Promise<Forges> {
  const response = await fetch("/api/v1/forges");
  if (response.status === 401) location.assign("/auth/login");
  if (response.status === 404) return { providers: [], can_add: false };
  if (!response.ok) throw new Error(`Could not load forges (${response.status})`);
  return response.json();
}
export const details = signal<Record<number, JobDetail>>({});
export const plans = signal<PlanView[]>([]);
export const connection = signal("Connecting");
export const error = signal("");
let pending: ReturnType<typeof setTimeout> | undefined;
let selected: number | undefined;
let operationsDirty = false;

async function read<T>(path: string): Promise<T> {
  const response = await fetch("/api/v1/" + path);
  if (response.status === 401) location.assign("/auth/login");
  if (!response.ok)
    throw new Error(`Could not load ${path} (${response.status})`);
  return response.json();
}
export async function selectJob(id: number) {
  selected = id;
  try {
    const detail = await read<JobDetail>(`jobs/${id}`);
    details.value = { ...details.value, [id]: detail };
  } catch (e) {
    error.value = String((e as Error).message);
  }
}
export async function refresh() {
  const nextPlans = await read<PlanView[]>("plans");
  batch(() => {
    plans.value = nextPlans;
    error.value = "";
  });
  if (selected) await selectJob(selected);
}
/** Job and intake changes refresh the operational pages too; streaming output does not rerun Doctor probes. */
export function markOperationsDirty() {
  operationsDirty = true;
}
export function scheduleRefresh() {
  if (!pending)
    pending = setTimeout(() => {
      pending = undefined;
      if (operationsDirty) { operationsDirty = false; invalidation.value++; }
      void refresh().catch((e: Error) => { error.value = e.message; });
    }, 100);
}
export function stopRefresh() {
  clearTimeout(pending);
  pending = undefined;
}
/** Who is signed in decides which controls are enabled; a 401 sends the browser to sign in. */
export function loadIdentity() {
  void read<typeof principal.value>("me")
    .then((user) => { principal.value = user; identityLoaded.value = true; })
    .catch((e: Error) => { error.value = e.message; });
}
export function mergeTail(tail: { id: number; worker: boolean; from: number; lines: string[] }) {
  const detail = details.value[tail.id];
  if (!detail) return;
  const key = tail.worker ? "worker" : "reviewer";
  const lines = detail[key].pushed === tail.from
    ? [...detail[`${key}_tail`], ...tail.lines]
    : tail.lines;
  details.value = {
    ...details.value,
    [tail.id]: {
      ...detail,
      [key]: { ...detail[key], pushed: tail.from + tail.lines.length },
      [`${key}_tail`]: lines.slice(-500),
    },
  };
}
