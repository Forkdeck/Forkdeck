import { signal } from "@preact/signals";
import { ByteBuffer } from "flatbuffers";
import { Frame, Body, Snapshot, Job, JobChanged, Automation, Tail } from "./wire/forkdeck/web.ts";
import { connection, error, refresh, scheduleRefresh, stopRefresh, mergeTail, markOperationsDirty } from "./state.ts";

export interface Board {
  seq: number;
  snapshot: Snapshot;
  jobs: Job[];
  automation: boolean;
}
export const board = signal<Board | null>(null);

export function apply(current: Board | null, frame: Frame): {
  next: Board | null;
  resync: boolean;
  tail?: { id: number; worker: boolean; from: number; lines: string[] };
} {
  if (frame.bodyType() === Body.Resync) return { next: current, resync: true };
  const seq = Number(frame.seq());
  if (frame.bodyType() === Body.Snapshot) {
    const snapshot: Snapshot = frame.body(new Snapshot());
    const jobs = Array.from({ length: snapshot.jobsLength() }, (_, i) => snapshot.jobs(i)!);
    return { next: { seq, snapshot, jobs, automation: snapshot.automation() }, resync: false };
  }
  if (!current) return { next: current, resync: false };
  // A snapshot reloaded mid-stream already holds everything up to its seq.
  if (seq <= current.seq) return { next: current, resync: false };
  const next = { ...current, seq };
  switch (frame.bodyType()) {
    case Body.JobChanged: {
      const changed: JobChanged = frame.body(new JobChanged());
      const job = changed.job()!;
      const index = current.jobs.findIndex((j) => j.id() === job.id());
      next.jobs = [...current.jobs];
      if (index < 0) next.jobs.push(job);
      else next.jobs[index] = job;
      break;
    }
    case Body.Automation: {
      const automation: Automation = frame.body(new Automation());
      next.automation = automation.on();
      break;
    }
    case Body.Tail: {
      const tail: Tail = frame.body(new Tail());
      return { next, resync: false, tail: {
        id: tail.id(), worker: tail.worker(), from: Number(tail.from()),
        lines: Array.from({ length: tail.linesLength() }, (_, i) => tail.lines(i)),
      } };
    }
  }
  return { next, resync: false };
}

function decode(bytes: ArrayBuffer): Frame {
  return Frame.getRootAsFrame(new ByteBuffer(new Uint8Array(bytes)));
}

export function connect(): () => void {
  let stopped = false;
  let socket: WebSocket | undefined;
  let retry: ReturnType<typeof setTimeout> | undefined;
  let delay = 1000;
  const abort = new AbortController();
  const failed = (e: unknown) => {
    if (stopped) return;
    error.value = (e as Error).message;
    connection.value = "Disconnected";
    reconnect();
  };
  const reconnect = () => {
    if (stopped || retry) return;
    retry = setTimeout(() => {
      retry = undefined;
      try {
        if (board.value && !needsSnapshot) open();
        else void load();
      } catch (e) { failed(e); }
    }, delay);
    delay = Math.min(delay * 2, 10000);
  };
  let needsSnapshot = true;
  // Slots busy, router counts, agents and jobs that left the daemon arrive in no event; a fresh
  // snapshot after each burst of frames carries them. One older than the board is dropped.
  let reloading: ReturnType<typeof setTimeout> | undefined;
  const reloadSoon = () => {
    if (reloading) return;
    reloading = setTimeout(async () => {
      reloading = undefined;
      try {
        const response = await fetch("/api/v1/snapshot", { signal: abort.signal });
        if (response.status === 401) { location.assign("/auth/login"); return; }
        if (!response.ok) return;
        const frame = decode(await response.arrayBuffer());
        if (stopped || frame.bodyType() !== Body.Snapshot) return;
        if (board.value && Number(frame.seq()) < board.value.seq) return;
        board.value = apply(board.value, frame).next;
      } catch {
        // A lost connection is reported by the socket.
      }
    }, 100);
  };
  const open = () => {
    if (stopped) return;
    const url = new URL("/api/v1/live", location.href);
    url.protocol = location.protocol === "https:" ? "wss:" : "ws:";
    url.searchParams.set("since", String(board.value!.seq));
    const ws = socket = new WebSocket(url);
    ws.binaryType = "arraybuffer";
    ws.onopen = () => {
      if (stopped || socket !== ws) return;
      delay = 1000;
      connection.value = "Live";
      error.value = "";
    };
    ws.onmessage = (event) => {
      if (stopped || socket !== ws) return;
      try {
        const frame = decode(event.data);
        const result = apply(board.value, frame);
        board.value = result.next;
        if (result.tail) mergeTail(result.tail);
        if (frame.bodyType() === Body.JobChanged || frame.bodyType() === Body.Automation) markOperationsDirty();
        if (frame.bodyType() !== Body.Snapshot) {
          scheduleRefresh();
          if (!result.resync) reloadSoon();
        }
        if (result.resync) {
          needsSnapshot = true;
          socket = undefined;
          ws.close();
          connection.value = "Reconnecting";
          void load();
        }
      } catch (e) {
        needsSnapshot = true;
        socket = undefined;
        ws.close();
        failed(e);
      }
    };
    ws.onerror = () => {
      if (!stopped && socket === ws) connection.value = "Reconnecting";
    };
    ws.onclose = () => {
      if (stopped || socket !== ws) return;
      socket = undefined;
      connection.value = "Reconnecting";
      reconnect();
    };
  };
  const load = async () => {
    try {
      const response = await fetch("/api/v1/snapshot", { signal: abort.signal });
      // A session that ended (sign-out, expiry) closes the socket; the reload lands here.
      if (response.status === 401) { stopped = true; location.assign("/auth/login"); return; }
      if (!response.ok) throw new Error(`Could not load snapshot (${response.status})`);
      const frame = decode(await response.arrayBuffer());
      if (stopped) return;
      if (frame.bodyType() !== Body.Snapshot) throw new Error("Expected board snapshot");
      board.value = apply(board.value, frame).next;
      needsSnapshot = false;
      open();
      void refresh().catch((e: Error) => { if (!stopped) error.value = e.message; });
    } catch (e) { failed(e); }
  };
  void load();
  return () => {
    stopped = true;
    abort.abort();
    clearTimeout(retry);
    clearTimeout(reloading);
    stopRefresh();
    socket?.close();
    connection.value = "Disconnected";
  };
}
