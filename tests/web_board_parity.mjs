// Run against --demo serve on 17441. Merge-only fields are fixtures in this test.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createRequire } from 'node:module';
import { mkdir } from 'node:fs/promises';
import { Frame, Body, Snapshot, Job, Lane, Agents, AgentWord, Router, LogEntry, Changed, Workspace } from '../web/src/wire/forkdeck/web.ts';
const require = createRequire(new URL('../web/package.json', import.meta.url));
const { chromium, expect } = require('@playwright/test');
const { Builder, ByteBuffer } = require('flatbuffers');
const base = process.env.FORKDECK_WEB_URL || 'http://127.0.0.1:17441';
const browser = await chromium.launch({ headless: true, executablePath: process.env.FORKDECK_CHROMIUM });
const artifactDir = process.env.FORKDECK_BOARD_ARTIFACTS || '/tmp/forkdeck-board-parity';
await mkdir(artifactDir, { recursive: true });
function snapshotFixture(bytes, plans) {
  const old = Frame.getRootAsFrame(new ByteBuffer(new Uint8Array(bytes))).body(new Snapshot());
  const b = new Builder(4096), str = s => b.createString(s || '');
  const jobs = Array.from({ length: old.jobsLength() }, (_, i) => old.jobs(i));
  if (!jobs.some(j => j.id() === 100)) jobs.push(null);
  const jobOffsets = jobs.map(j => {
    const id = j?.id() ?? 100;
    const fields = ['title','lane','state','meta','elapsed','route','warn','status','agent'].map(k => str(j?.[k]() || (k === 'title' ? plans.find(p => p.id === 100).title : k === 'lane' ? 'Attention' : '')));
    const kind = str(id === 100 ? 'plan' : j.kind() || 'manual');
    const stages = Job.createStagesVector(b, j?.stagesArray() || []);
    Job.startJob(b); Job.addId(b,id);
    ['Title','Lane','State','Meta','Elapsed','Route','Warn','Status','Agent'].forEach((k,i) => Job[`add${k}`](b,fields[i]));
    Job.addKind(b,kind); Job.addStages(b,stages); Job.addQueued(b,id === 73 ? 1 : 0);
    return Job.endJob(b);
  });
  const jobVector = Snapshot.createJobsVector(b,jobOffsets);
  const lanes = Snapshot.createLanesVector(b,Array.from({length:old.lanesLength()},(_,i) => {
    const lane = old.lanes(i); return Lane.createLane(b,str(lane.id()),str(lane.id() === 'Attention' ? 'Needs attention' : lane.label()));
  }));
  const words = Agents.createWordsVector(b,[['claude','ON CALL'],['codex','VIA COPILOT'],['copilot','NO ACCESS']].map(([a,w]) => AgentWord.createAgentWord(b,str(a),str(w))));
  Agents.startAgents(b); Agents.addWords(b,words); Agents.addClaude(b,old.agents()?.claude() ?? false); Agents.addCodex(b,old.agents()?.codex() ?? false); Agents.addCopilot(b,old.agents()?.copilot() ?? false); const agents = Agents.endAgents(b);
  const router = Router.createRouter(b,str('jev'),12n,42n,str('JEV DOWN · RULES'),2,3n,4n,str('http://router.invalid'));
  const log = Snapshot.createLogVector(b,[LogEntry.createLogEntry(b,str('12:00'),str('issues'),str('Session fixture line'))]);
  const workspace = str(old.workspace());
  const workspaces = Snapshot.createWorkspacesVector(b,Array.from({length:old.workspacesLength()},(_,i) => {
    const item = old.workspaces(i); return Workspace.createWorkspace(b,str(item.id()),str(item.name()));
  }));
  Snapshot.startSnapshot(b); Snapshot.addJobs(b,jobVector); Snapshot.addLanes(b,lanes); Snapshot.addAgents(b,agents); Snapshot.addRouter(b,router); Snapshot.addLog(b,log); Snapshot.addWorkspace(b,workspace); Snapshot.addWorkspaces(b,workspaces); Snapshot.addBusy(b,old.busy()); Snapshot.addCapacity(b,old.capacity()); Snapshot.addAutomation(b,old.automation());
  Snapshot.addRunning(b,2); Snapshot.addQueued(b,3); Snapshot.addSpentJobs(b,17); Snapshot.addSpentTokens(b,12345n); Snapshot.addDemo(b,true);
  b.finish(Frame.createFrame(b,9000n,Body.Snapshot,Snapshot.endSnapshot(b)));
  return Buffer.from(b.asUint8Array());
}
let detail;
async function pageFor(width = 1440, fixture = true) {
  const page = await browser.newPage({ viewport: { width, height: 1000 } });
  if (fixture) {
    page.fixtureLines = 120;
    let socket, seq = 9000;
    await page.routeWebSocket('**/api/v1/live?*', ws => { socket = ws; });
    page.append = () => {
      page.fixtureLines += 3;
      const b = new Builder(128); Changed.startChanged(b); const body = Changed.endChanged(b);
      b.finish(Frame.createFrame(b,BigInt(++seq),Body.Changed,body)); socket.send(Buffer.from(b.asUint8Array()));
    };
    const plans = await (await page.request.get(`${base}/api/v1/plans`)).json();
    const bytes = await (await page.request.get(`${base}/api/v1/snapshot`)).body();
    const snap = snapshotFixture(bytes,plans);
    await page.route('**/api/v1/snapshot', route => route.fulfill({ contentType:'application/octet-stream',body:snap }));
    await page.route('**/api/v1/jobs/73', async route => {
      const response = await route.fetch();
      detail = await response.json();
      detail.source = { url:'https://example.org/issues/73', branch:'parity-branch', workdir:'/tmp/parity-workdir' };
      detail.board.log = Array.from({length:page.fixtureLines},(_,i) => ['', 'worker', `Worker line ${i}`]);
      detail.board.log.push(['','you','Conversation ask'],['','agent','Conversation answer']);
      detail.decision.gates = [{name:'complexity',value:0.75,sharp:false,answer:'high'}];
      detail.decision.note = 'Routing fixture note';
      detail.findings = ['Finding fixture'];
      await route.fulfill({json:detail});
    });
  }
  return page;
}
try {
  await test('header, status, cards and session log consume snapshot fields', async () => {
    const page = await pageFor();
    try {
      await page.goto(base);
      await expect(page.locator('.brand')).toContainText('(demo)');
      for (const text of ['CLAUDE ON CALL','CODEX VIA COPILOT','COPILOT NO ACCESS','JEV DOWN · RULES']) await expect(page.getByText(text,{exact:true}).first()).toBeVisible();
      await expect(page.locator('.board-status')).toContainText('running: [2]');
      await expect(page.locator('.board-status')).toContainText('queued: [3]');
      await expect(page.locator('.board-status')).toContainText('24h: 17 jobs / 12345 tokens');
      await expect(page.locator('.router-status')).toContainText('fallbacks 3');
      await expect(page.locator('.router-status')).toContainText('memory hits 4');
      await expect(page.locator('.job-card').filter({hasText:'#73'})).toContainText('✉1');
      await page.getByText('Session log',{exact:true}).click();
      await expect(page.locator('.session-log')).toContainText('Session fixture line');
      await page.locator('.job-card').filter({hasText:'#100'}).click();
      await expect(page).toHaveURL(`${base}/plans/100`);
    } finally { await page.close(); }
  });
  await test('plan job URLs redirect and unknown jobs never select another job', async () => {
    const page = await pageFor();
    try {
      await page.goto(`${base}/jobs/100`); await expect(page).toHaveURL(`${base}/plans/100`);
      await page.goto(`${base}/jobs/999`);
      await expect(page.getByText('Job #999 is not on the board',{exact:true})).toBeVisible();
      assert.equal(await page.locator('.detail .job-actions').count(),0);
    } finally { await page.close(); }
  });
  await test('detail renders routing, stages, outcome and conversation; follows until scrolled', async () => {
    const page = await pageFor();
    try {
      await page.goto(`${base}/jobs/73`);
      await expect(page.locator('.detail')).toContainText('parity-branch');
      await expect(page.locator('.detail')).toContainText('/tmp/parity-workdir');
      await expect(page.locator('.routing-gates')).toContainText('complexity');
      await expect(page.locator('.routing-gates')).toContainText('split');
      await expect(page.locator('.detail')).toContainText('Routing fixture note');
      await expect(page.locator('.stage-details')).toHaveCount(2);
      await expect(page.locator('.outcome')).toContainText('Finding fixture');
      const log = page.locator('.detail .log');
      await expect.poll(() => log.evaluate(el => el.scrollHeight-el.clientHeight-el.scrollTop)).toBeLessThan(3);
      page.append();
      await expect(log).toContainText('Worker line 122');
      await expect.poll(() => log.evaluate(el => el.scrollHeight-el.clientHeight-el.scrollTop)).toBeLessThan(3);
      await page.keyboard.press('PageUp');
      await expect(page.getByRole('button',{name:/Jump to latest/})).toBeVisible();
      const before = await log.evaluate(el => el.scrollTop);
      page.append();
      await expect(log).toContainText('Worker line 125');
      assert.equal(await log.evaluate(el => el.scrollTop),before);
      await page.getByRole('button',{name:/Jump to latest/}).click();
      await expect.poll(() => log.evaluate(el => el.scrollHeight-el.clientHeight-el.scrollTop)).toBeLessThan(3);
      await page.keyboard.press('PageUp');
      const paged = await log.evaluate(el => el.scrollTop);
      await page.keyboard.press('PageDown');
      await expect.poll(() => log.evaluate(el => el.scrollTop)).toBeGreaterThan(paged);
      await page.keyboard.press('End');
      await expect.poll(() => log.evaluate(el => el.scrollHeight-el.clientHeight-el.scrollTop)).toBeLessThan(3);
      await page.getByRole('tab',{name:'Conversation',exact:true}).click();
      await expect(log).toContainText('Conversation ask'); await expect(log).toContainText('Conversation answer');
      await expect(log).not.toContainText('Worker line');
    } finally { await page.close(); }
  });
  await test('output paging and End work while the reply box has focus, like the TUI', async () => {
    const page = await pageFor();
    try {
      await page.goto(`${base}/jobs/73`);
      await page.locator('.job-actions').getByRole('textbox').focus();
      await page.keyboard.press('PageUp');
      await expect(page.getByRole('button',{name:/Jump to latest/})).toBeVisible();
      await page.keyboard.press('End');
      await expect.poll(() => page.locator('.detail .log').evaluate(el => el.scrollHeight-el.clientHeight-el.scrollTop)).toBeLessThan(3);
    } finally { await page.close(); }
  });
  await test('phone shows router facts and plan cards navigate to plans', async () => {
    const page = await pageFor(390);
    try {
      await page.goto(base);
      await expect(page.locator('.phone-brand')).toContainText('(demo)');
      await expect(page.locator('.phone')).toContainText('CLAUDE ON CALL');
      await page.locator('.phone-router summary').click();
      await expect(page.locator('.router-status')).toContainText('memory hits 4');
      await page.getByRole('tab',{name:'All',exact:true}).click();
      await expect(page.locator('.phone-card').filter({hasText:'#73'})).toContainText('✉1');
      await page.locator('.phone-card').filter({hasText:'#100'}).click();
      await expect(page).toHaveURL(`${base}/plans/100`);
    } finally { await page.close(); }
  });
  for (const width of [1440,960,390]) await test(`board and plans remain reachable at ${width}px`, async () => {
    const page = await pageFor(width);
    try {
      await page.goto(base);
      await expect(page.locator(width < 700 ? '.phone-card' : '.job-card').first()).toBeVisible();
      await page.screenshot({path:`${artifactDir}/board-${width}.png`});
      if (width < 700) {
        await page.getByRole('tab',{name:'Needs attention',exact:true}).click();
        await expect(page.getByRole('tab',{name:'Needs attention',exact:true})).toHaveAttribute('aria-selected','true');
        await page.getByRole('tab',{name:'All',exact:true}).click();
      }
      await page.goto(`${base}/jobs/73`);
      await expect(page.locator('.detail-title')).toBeVisible();
      if (width < 700) { await page.getByRole('link',{name:'← Board',exact:true}).click(); await expect(page).toHaveURL(`${base}/`); await page.goto(`${base}/jobs/73`); }
      await page.screenshot({path:`${artifactDir}/detail-${width}.png`});
      await page.goto(`${base}/plans/100`);
      await expect(page.locator('.plan-title')).toBeVisible();
      assert.equal(await page.locator('.plans-sidebar .plan-link,.plans-sidebar select').count(),0);
      await expect(page.locator('.plans-main .issue-plan-list')).toContainText('100');
      assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
      await page.screenshot({path:`${artifactDir}/plans-${width}.png`});
    } finally { await page.close(); }
  });
  await test('unextended server fields degrade cleanly', async () => {
    const page = await pageFor(1440,false);
    try { await page.goto(`${base}/jobs/73`); await expect(page.locator('.detail-title')).toBeVisible(); await expect(page.locator('.detail')).not.toContainText('undefined'); }
    finally { await page.close(); }
  });
} finally { await browser.close(); }
