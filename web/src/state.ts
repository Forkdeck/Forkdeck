import { signal, batch } from "@preact/signals";
import type {
  BoardProjection,
  JobSummary,
  JobDetail as NativeJobDetail,
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
export const details = signal<Record<number, JobDetail>>({});
export const plans = signal<PlanView[]>([]);
export const connection = signal("Connecting");
export const error = signal("");
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
export async function refresh() {
  const nextPlans = await read<PlanView[]>("plans");
  batch(() => {
    plans.value = nextPlans;
    error.value = "";
  });
  if (selected) await selectJob(selected);
}
export function scheduleRefresh() {
  if (!pending)
    pending = setTimeout(() => {
      pending = undefined;
      void refresh().catch((e: Error) => { error.value = e.message; });
    }, 100);
}
export function stopRefresh() {
  clearTimeout(pending);
  pending = undefined;
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
