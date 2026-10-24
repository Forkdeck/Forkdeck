import type { Job } from "./wire/forkdeck/web.ts";
export interface PresentedJob {
  job: Job;
  group: string;
}
export function present(job: Job): PresentedJob {
  const lane = job.lane() ?? "";
  return { job, group: ["Build", "Review", "Delivery", "Queue"].includes(lane) ? "Active" : lane };
}
export function pill(state: string | null): string {
  return `pill tone-${(state ?? "").replaceAll(" ", "-").toLowerCase()}`;
}
export function stageChip(state: number, index: number) {
  const mark = ["○ ", "● ", "✓ ", "– ", "✕ "][state] ?? "○ ";
  return {
    label: mark + ["Route", "Work", "Checks", "Review", "Deliver"][index],
    className: `stage-chip stage-${state}`,
  };
}
export function tabClass(selected: boolean): string {
  return `tab${selected ? " selected" : ""}`;
}
