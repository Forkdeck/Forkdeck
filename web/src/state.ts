import { signal, batch, computed } from "@preact/signals";
import type { PresentedJob } from "./ui";
import type {
  BoardProjection,
  JobSummary,
  JobDetail as NativeJobDetail,
  Snapshot as NativeSnapshot,
  Event as FeedEvent,
} from "./contracts";

export interface BoardJob extends JobSummary, BoardProjection {}
export interface JobDetail extends NativeJobDetail {
  board: BoardJob;
  revision: string;
  choices: { label: string; detail: string }[];
  queued: string[];
  actions: { reply: boolean; stop: boolean; retry: boolean; resume: boolean };
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
interface Snapshot extends Omit<NativeSnapshot, "jobs"> {
  jobs: BoardJob[];
  workspace: string;
  workspaces: { id: string; name: string }[];
  capacity: number;
  busy: number;
  router: { kind: string; calls: number; last_ms: number };
  agents: { claude: boolean; codex: boolean; copilot: boolean };
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
export const invalidation = signal(0);
export const writeReason = computed(() => !identityLoaded.value ? "Checking permissions…" : principal.value?.role === "viewer" ? "Operator or admin role required" : "");
export async function action<T = { id?: number }>(command: Record<string, unknown>): Promise<T | undefined> {
  if (actionBusy.value || writeReason.value) return;
  actionBusy.value = true;
  actionError.value = "";
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
export const snapshot = signal<Snapshot | null>(null);
export const details = signal<Record<number, JobDetail>>({});
export const plans = signal<PlanView[]>([]);
export const connection = signal("Connecting");
export const error = signal("");
let source: EventSource | undefined;
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
async function refresh() {
  const snap = await read<Snapshot>("snapshot");
  const nextPlans = await read<PlanView[]>("plans");
  batch(() => {
    snapshot.value = snap;
    plans.value = nextPlans;
    error.value = "";
  });
  if (selected) await selectJob(selected);
  return snap.seq;
}
function scheduleRefresh() {
  if (!pending)
    pending = setTimeout(() => {
      pending = undefined;
      if (operationsDirty) { operationsDirty = false; invalidation.value++; }
      void refresh().catch(fail);
    }, 100);
}
function fail(e: unknown) {
  error.value = (e as Error).message;
  connection.value = "Disconnected";
}

/** Fold native feed events into signals; enriched projections are refreshed in one bounded batch. */
export function reduce(event: FeedEvent) {
  const snap = snapshot.value;
  if (!snap) return;
  if (event === "Resync") {
    reconnect();
    return;
  }
  if (typeof event !== "object") return;
  if ("Automation" in event)
    snapshot.value = {
      ...snap,
      automation: (event.Automation as { on: boolean }).on,
    };
  if ("Job" in event) {
    const job = (event.Job as { job: Partial<BoardJob> }).job;
    snapshot.value = {
      ...snap,
      jobs: snap.jobs.map((j) => (j.id === job.id ? { ...j, ...job } : j)),
    };
  }
  if ("Tail" in event) {
    const tail = event.Tail as {
      id: number;
      which: "Worker" | "Reviewer";
      from: number;
      lines: string[];
    };
    const detail = details.value[tail.id];
    if (detail) {
      const key = tail.which === "Worker" ? "worker" : "reviewer";
      const lines =
        detail[key].pushed === tail.from
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
  }
  // Streaming tail lines refresh job projections, without rerunning Doctor probes per line.
  operationsDirty ||= "Job" in event || "Automation" in event ||
    ("Log" in event && event.Log.lines.some(line => line.who === "web"));
  scheduleRefresh();
}
function connect(seq: number) {
  source?.close();
  // EventSource cannot set Last-Event-ID on its first request. Establish snapshot -> subscription
  // with a query cursor; native reconnections send the standard Last-Event-ID header.
  source = new EventSource(`/api/v1/events?since=${seq}`);
  source.onopen = () => {
    connection.value = "Live";
  };
  source.onerror = () => {
    connection.value = "Reconnecting";
    void read<typeof principal.value>("me").catch(fail);
  };
  source.onmessage = (e) => {
    reduce(JSON.parse(e.data));
  };

}
function reconnect() {
  source?.close();
  void refresh().then(connect).catch(fail);
}
export function start() {
  void read<typeof principal.value>("me").then((user) => { principal.value = user; identityLoaded.value = true; }).catch(fail);
  reconnect();
  return () => {
    source?.close();
    clearTimeout(pending);
  };
}

export function present(job: BoardJob): PresentedJob {
  return {
    ...job,
    group: ["Build", "Review", "Delivery", "Queue"].includes(job.lane)
      ? "Active"
      : job.lane,
  };
}
