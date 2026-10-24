import { signal, batch } from "@preact/signals";
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
}
export interface PlanView {
  id: number | string;
  title: string;
  subtitle?: string;
  status: string;
  scheduler: string;
  description: string;
  decision?: { number: number; title: string };
  plan: {
    repo: string;
    revision: number;
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
  capacity: number;
  busy: number;
  router: { kind: string; calls: number; last_ms: number };
  agents: { claude: boolean; codex: boolean; copilot: boolean };
}
export const snapshot = signal<Snapshot | null>(null);
export const details = signal<Record<number, JobDetail>>({});
export const plans = signal<PlanView[]>([]);
export const connection = signal("Connecting");
export const error = signal("");
let source: EventSource | undefined;
let pending: ReturnType<typeof setTimeout> | undefined;
let selected: number | undefined;

async function read<T>(path: string): Promise<T> {
  const response = await fetch("/api/v1/" + path);
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
  };
  source.onmessage = (e) => {
    reduce(JSON.parse(e.data));
  };
  source.addEventListener("freshness", scheduleRefresh);
}
function reconnect() {
  source?.close();
  void refresh().then(connect).catch(fail);
}
export function start() {
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
