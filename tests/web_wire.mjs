// Node 24 strips TypeScript types; run: node tests/web_wire.mjs
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { test } from 'node:test';
import { apply, board, connect } from '../web/src/live.ts';
import { connection } from '../web/src/state.ts';
import { Frame, Body, Snapshot, Job, JobChanged, Automation, Tail, Resync, Changed } from '../web/src/wire/forkdeck/web.ts';
const require = createRequire(new URL('../web/package.json', import.meta.url));
const { Builder, ByteBuffer } = require('flatbuffers');

function frame(type, seq, build) {
  const b = new Builder(1024);
  const body = build(b);
  b.finish(Frame.createFrame(b, BigInt(seq), type, body));
  return Frame.getRootAsFrame(new ByteBuffer(b.asUint8Array()));
}
function job(b, id, state = 'working', lane = 'Build') {
  return Job.createJob(b, id, b.createString('Plans — sync · 🚀'), b.createString(lane), b.createString(state), 0, 0, 0, Job.createStagesVector(b, [2,1,0,0,0]), 0, 0);
}
function snapshot(seq = 7) {
  return frame(Body.Snapshot, seq, b => {
    const jobs = Snapshot.createJobsVector(b, [job(b, 71), job(b, 72)]);
    Snapshot.startSnapshot(b);
    Snapshot.addAutomation(b, true);
    Snapshot.addJobs(b, jobs);
    return Snapshot.endSnapshot(b);
  });
}
const initial = () => apply(null, snapshot()).next;

test('snapshot retains tables and JobChanged replaces in board order', () => {
  const current = initial();
  assert.equal(current.seq, 7);
  assert.ok(current.snapshot instanceof Snapshot);
  assert.ok(current.jobs[1] instanceof Job);
  assert.equal(current.jobs[1].bb, current.snapshot.bb);
  assert.equal(current.jobs[1].title(), 'Plans — sync · 🚀');
  assert.deepEqual([...current.jobs[1].stagesArray()], [2,1,0,0,0]);
  const change = frame(Body.JobChanged, 8, b => JobChanged.createJobChanged(b, job(b, 72, 'done')));
  const next = apply(current, change).next;
  assert.deepEqual(next.jobs.map(j => j.id()), [71,72]);
  assert.equal(next.jobs[1].state(), 'done');
  assert.equal(next.jobs[1].bb, change.bb);
  assert.equal(current.jobs[1].state(), 'working');
  assert.equal(next.seq, 8);
});
test('unknown jobs append and survive automation changes', () => {
  const change = frame(Body.JobChanged, 8, b => JobChanged.createJobChanged(b, job(b, 900, 'queued', 'Queue')));
  const current = apply(initial(), change).next;
  const next = apply(current, frame(Body.Automation, 9, b => Automation.createAutomation(b, false))).next;
  assert.deepEqual(next.jobs.map(j => j.id()), [71,72,900]);
  assert.equal(next.jobs[2].lane(), 'Queue');
  assert.equal(next.automation, false);
  assert.equal(next.seq, 9);
});
test('resync preserves board even when daemon sequence has gone backwards', () => {
  const current = initial();
  const result = apply(current, frame(Body.Resync, 1, b => { Resync.startResync(b); return Resync.endResync(b); }));
  assert.equal(result.resync, true);
  assert.equal(result.next, current);
  const restarted = apply(current, snapshot(2)).next;
  assert.equal(restarted.seq, 2);
  assert.equal(restarted.jobs.length, 2);
});
test('tail returns detail data and advances resume cursor', () => {
  const result = apply(initial(), frame(Body.Tail, 10, b => Tail.createTail(b, 72, true, 500n, Tail.createLinesVector(b, [b.createString('line — 🚀'), b.createString('next')]))));
  assert.deepEqual(result.tail, { id:72, worker:true, from:500, lines:['line — 🚀','next'] });
  assert.equal(result.next.seq, 10);
});
test('Changed advances the cursor without replacing retained tables', () => {
  const current = initial();
  const result = apply(current, frame(Body.Changed, 11, b => { Changed.startChanged(b); return Changed.endChanged(b); }));
  assert.equal(result.next.seq, 11);
  assert.equal(result.next.jobs, current.jobs);
  assert.equal(result.next.snapshot, current.snapshot);
  assert.equal(result.resync, false);
});

test('socket retries resume, back off, reload on Resync and stop cleanly', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const sockets = [], requests = [];
  let rejectSocket = false;
  class Socket {
    constructor(url) {
      if (rejectSocket) throw new Error('Cannot open socket');
      this.url = String(url);
      sockets.push(this);
    }
    close() { this.closed = true; this.onclose?.(); }
    message(frame) {
      const bytes = frame.bb.bytes();
      this.onmessage({ data: bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) });
    }
  }
  t.mock.method(globalThis, 'fetch', async (url) => {
    requests.push(url);
    if (url === '/api/v1/plans') return { ok:true, json:async()=>[] };
    const bytes = snapshot(requests.filter(p => p === '/api/v1/snapshot').length === 1 ? 7 : 2).bb.bytes();
    return { ok:true, arrayBuffer:async()=>bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) };
  });
  const savedLocation = Object.getOwnPropertyDescriptor(globalThis, 'location');
  const savedSocket = Object.getOwnPropertyDescriptor(globalThis, 'WebSocket');
  Object.defineProperty(globalThis, 'location', { configurable:true, value:new URL('https://localhost:17420/') });
  Object.defineProperty(globalThis, 'WebSocket', { configurable:true, value:Socket });
  const settle = async () => { for (let i=0; i<12; i++) await Promise.resolve(); };
  board.value = null;
  const stop = connect();
  try {
    await settle();
    assert.equal(sockets[0].url, 'wss://localhost:17420/api/v1/live?since=7');
    assert.equal(sockets[0].binaryType, 'arraybuffer');
    sockets[0].onopen();
    assert.equal(connection.value, 'Live');
    sockets[0].message(frame(Body.Automation, 8, b => Automation.createAutomation(b, false)));
    sockets[0].close();
    assert.equal(connection.value, 'Reconnecting');
    t.mock.timers.tick(999);
    assert.equal(sockets.length, 1);
    rejectSocket = true;
    t.mock.timers.tick(1);
    await settle();
    assert.equal(connection.value, 'Disconnected');
    rejectSocket = false;
    t.mock.timers.tick(1999);
    assert.equal(sockets.length, 1);
    t.mock.timers.tick(1);
    assert.match(sockets[1].url, /since=8$/);
    assert.equal(requests.filter(p => p === '/api/v1/snapshot').length, 1);
    sockets[1].onopen();
    sockets[1].message(frame(Body.Resync, 1, b => { Resync.startResync(b); return Resync.endResync(b); }));
    await settle();
    assert.equal(sockets[1].closed, true);
    assert.match(sockets[2].url, /since=2$/);
    assert.equal(board.value.seq, 2);
    assert.equal(requests.filter(p => p === '/api/v1/snapshot').length, 2);
    stop();
    t.mock.timers.tick(10000);
    assert.equal(sockets.length, 3);
    assert.equal(sockets[2].closed, true);
    assert.equal(connection.value, 'Disconnected');
  } finally {
    stop();
    for (const [key, descriptor] of [['location',savedLocation],['WebSocket',savedSocket]]) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
    board.value = null;
  }
});
