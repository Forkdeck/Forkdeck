import type { BoardJob } from "./state";
export interface PresentedJob extends BoardJob {
  group: string;
}
export function pill(state: string): string {
  return `pill tone-${state.replaceAll(" ", "-").toLowerCase()}`;
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
