// FORKDECK_WEB_URL=http://127.0.0.1:17442 FORKDECK_CHROMIUM=... node tests/web_keys.mjs
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { Frame, Snapshot } from '../web/src/wire/forkdeck/web.ts';
const require = createRequire(new URL('../web/package.json', import.meta.url));
const { ByteBuffer } = require('flatbuffers');
const { chromium, expect } = require('@playwright/test');
const base = process.env.FORKDECK_WEB_URL || 'http://127.0.0.1:17442';
const browser = await chromium.launch({ headless: true, executablePath: process.env.FORKDECK_CHROMIUM });
let passed = 0;
const failures = [];
async function test(name, run) {
  const page = await browser.newPage({ viewport: { width: 1440, height: 960 } });
  page.setDefaultTimeout(3000);
  const errors = [];
  page.on('pageerror', e => errors.push(String(e)));
  try { await run(page); assert.deepEqual(errors, []); passed++; console.log(`PASS ${name}`); }
  catch (e) { failures.push(name); console.error(`FAIL ${name}: ${e.message}`); }
  finally { await page.close(); }
}
async function board(page) {
  await page.goto(base);
  await page.locator('.job-card').first().waitFor();
  await expect(page.getByRole('textbox', { name: 'New task', exact: true })).toBeEnabled();
}
// Server-worker additions stay in test routes, never in browser source.
async function fixture(page, change = v => v) {
  const commands = [];
  await page.route('**/api/v1/jobs/*', async route => {
    const response = await route.fetch();
    if (!response.ok()) { await route.fulfill({ response }); return; }
    await route.fulfill({ response, json: change(await response.json()) });
  });
  await page.route('**/api/v1/actions', async route => {
    commands.push(route.request().postDataJSON());
    // A typed task of the seeded board: id 100 is a planning job, whose page is /plans/100.
    await route.fulfill({ json: { id: 70 } });
  });
  return commands;
}
async function openTyped(page) {
  await board(page);
  const ids = await page.locator('.job-card .job-heading > span:first-child').allTextContents();
  for (const label of ids) {
    const id = Number(label.slice(1));
    const detail = await (await page.request.get(`${base}/api/v1/jobs/${id}`)).json();
    if (detail.actions.reply && !detail.actions.stop) {
      await page.goto(`${base}/jobs/${id}`);
      // A planning job's card leads to its plan page, which has no reply box.
      await page.waitForTimeout(200);
      if (!new URL(page.url()).pathname.startsWith('/jobs/')) continue;
      await page.locator('.reply-input').waitFor();
      return id;
    }
  }
  throw new Error('No finished typed task in seeded board');
}
try {
  await test('composer plan keys, reset, history, clear and workspace storage', async page => {
    const commands = await fixture(page);
    await board(page);
    const input = page.getByRole('textbox', { name: 'New task', exact: true });
    const plan = page.getByRole('checkbox', { name: 'Plan first' });
    await expect(plan).not.toBeChecked();
    await input.fill('first submitted task');
    await input.press('Control+p');
    await expect(plan).toBeChecked();
    await input.press('Shift+Tab');
    await expect(plan).not.toBeChecked();
    await input.press('Control+p');
    await input.press('Enter');
    await expect.poll(() => commands.length).toBe(1);
    assert.equal(commands[0].plan_first, true);
    await expect(plan).not.toBeChecked();
    await input.fill('unfinished draft');
    await input.press('ArrowUp');
    await expect(input).toHaveValue('first submitted task');
    await input.press('ArrowDown');
    await expect(input).toHaveValue('unfinished draft');
    await page.reload();
    await expect(input).toHaveValue('unfinished draft');
    await input.press('Control+u');
    await expect(input).toHaveValue('');
    await input.fill('clear me');
    await input.press('Escape');
    await expect(input).toHaveValue('');
    await input.fill('line one\nline two');
    await input.evaluate(el => el.setSelectionRange(10, 10));
    await input.press('ArrowUp');
    await expect(input).toHaveValue('line one\nline two');
    const spaces = page.locator('.writable-composer select').first();
    const options = await spaces.locator('option').evaluateAll(els => els.map(el => el.value));
    if (options.length > 1) {
      await spaces.selectOption(options[1]);
      await expect(input).toHaveValue('');
      await spaces.selectOption(options[0]);
      await expect(input).toHaveValue('line one\nline two');
    }
  });
  await test('reply title, Ctrl-S/Ctrl-Enter, choices with draft and empty Enter', async page => {
    const commands = await fixture(page, detail => ({ ...detail, prompt_title: 'Server supplied prompt', choices: [{ label: 'Option A', detail: 'An approach' }] }));
    const id = await openTyped(page);
    const input = page.getByRole('textbox', { name: 'Server supplied prompt', exact: true });
    await expect(input).toHaveAttribute('placeholder', 'Server supplied prompt');
    await input.fill('send immediately');
    await input.press('Control+s');
    await expect.poll(() => commands.length).toBe(1);
    assert.equal(commands[0].action, 'steer');
    assert.equal(commands[0].id, id);
    assert.equal(commands[0].text, 'send immediately');
    await input.fill('also immediate');
    await input.press('Control+Enter');
    await expect.poll(() => commands.length).toBe(2);
    assert.equal(commands[1].action, 'steer');
    await input.fill('keep this guidance');
    await page.getByRole('button', { name: 'Option A An approach' }).click();
    await expect.poll(() => commands.length).toBe(3);
    assert.equal(commands[2].text, 'keep this guidance');
    assert.equal(commands[2].option, 0);
    await expect(input).toBeEnabled();
    await expect(input).toHaveValue('');
    await input.press('Enter');
    await expect.poll(() => commands.length).toBe(4);
    assert.equal(commands[3].action, 'reply');
    assert.equal(commands[3].text, '');
  });
  await test('fallback and refused actions preserve draft and show error', async page => {
    await fixture(page, detail => { delete detail.prompt_title; return detail; });
    await openTyped(page);
    await page.route('**/api/v1/actions', route => route.fulfill({ status: 400, json: { error: 'Enter a message' } }));
    const input = page.locator('.reply-input');
    await expect(input).toHaveAttribute('placeholder', 'Type a follow-up…');
    await input.press('Enter');
    await expect(page.getByRole('alert')).toHaveText('Enter a message');
    await input.fill('preserved');
    await input.press('Control+s');
    await expect(page.getByRole('alert')).toHaveText('Enter a message');
    await expect(input).toHaveValue('preserved');
  });
  await test('global navigation, selection guards, intake, choices and browser help', async page => {
    const commands = await fixture(page, detail => ({ ...detail, actions: { ...detail.actions, stop: true }, choices: [{ label: 'Option A', detail: 'An approach' }] }));
    await board(page);
    await page.keyboard.press('n');
    const input = page.getByRole('textbox', { name: 'New task', exact: true });
    await expect(input).toBeFocused();
    await input.fill('editable text');
    await input.press('Control+a');
    assert.equal(commands.length, 0);
    await input.press('Control+x');
    assert.equal(commands.length, 0);
    await page.locator('.brand').click();
    await page.keyboard.press('Control+a');
    await expect.poll(() => commands.length).toBe(1);
    assert.equal(commands[0].action, 'automation');
    // No panel open means no visible selection: Ctrl-X stops nothing.
    await page.keyboard.press('Control+x');
    await page.waitForTimeout(300);
    assert.equal(commands.length, 1);
    await page.keyboard.press('F2');
    await expect(page).toHaveURL(/\/jobs\/\d+$/);
    const selected = Number(new URL(page.url()).pathname.split('/').pop());
    await page.locator('.choices button').first().waitFor();
    await page.keyboard.press('Control+x');
    await expect.poll(() => commands.length).toBe(2);
    assert.equal(commands[1].id, selected);
    await page.keyboard.press('F5');
    await expect(page.locator('.choices button').first()).toBeFocused();
    const bytes = await (await page.request.get(`${base}/api/v1/snapshot`)).body();
    const snap = Frame.getRootAsFrame(new ByteBuffer(bytes)).body(new Snapshot());
    const ids = Array.from({ length: snap.jobsLength() }, (_, i) => snap.jobs(i).id());
    await page.keyboard.press('Alt+ArrowUp');
    const previous = ids[Math.max(0, ids.indexOf(selected) - 1)];
    await expect(page).toHaveURL(new RegExp(`/jobs/${previous}$`));
    await page.keyboard.press('Alt+ArrowDown');
    await expect(page).toHaveURL(new RegExp(`/jobs/${ids[Math.min(ids.length - 1, ids.indexOf(previous) + 1)]}$`));
    for (const key of ['Alt+ArrowLeft', 'Alt+ArrowRight']) {
      const expected = await page.evaluate(key => {
        const cards = [...document.querySelectorAll('.lane')].map(lane => [...lane.querySelectorAll('.job-card')].map(card => Number(card.querySelector('.job-heading > span').textContent.slice(1))));
        const id = Number(location.pathname.split('/').pop());
        const from = cards.findIndex(lane => lane.includes(id));
        const step = key.endsWith('Right') ? 1 : -1;
        for (let i = from + step; i >= 0 && i < cards.length; i += step) if (cards[i].length) return cards[i].at(-1);
        return id;
      }, key);
      await page.keyboard.press(key);
      // Planning jobs open their plan, like their board card.
      await expect(page).toHaveURL(new RegExp(`/(jobs|plans)/${expected}$`));
      if (new URL(page.url()).pathname.startsWith('/plans/')) await page.goBack();
    }
    await page.keyboard.press('F2');
    await expect(page).toHaveURL(`${base}/`);
    await page.keyboard.press('F4');
    await expect(page).toHaveURL(`${base}/repos`);
    await page.keyboard.press('F6');
    await expect(page).toHaveURL(`${base}/plans`);
    await page.goto(base);
    await page.keyboard.press('?');
    await expect(page.getByRole('dialog')).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Keyboard shortcuts', exact: true })).toBeInViewport();
    for (const key of ['Ctrl-T', 'Ctrl-N', 'Ctrl-W', 'Ctrl-R', 'Ctrl-C', 'Tab', 'PgUp', 'Ctrl-P', 'Ctrl-U']) await expect(page.getByRole('dialog')).toContainText(key);
    await page.screenshot({ path: '/tmp/forkdeck-keys-help-1440.png' });
    await page.keyboard.press('Escape');
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await expect(page.getByRole('contentinfo', { name: 'Keyboard shortcuts' })).toBeVisible();
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(page.getByRole('contentinfo', { name: 'Keyboard shortcuts' })).toHaveCount(0);
    await page.screenshot({ path: '/tmp/forkdeck-keys-board-390.png' });
  });
  await test('finished typed task links a plan and displays server refusal', async page => {
    const commands = await fixture(page);
    const id = await openTyped(page);
    await page.getByRole('button', { name: 'Plan as issue…' }).click();
    await page.getByRole('textbox', { name: 'Planning request', exact: true }).fill('Plan this existing task');
    await page.getByRole('textbox', { name: 'Existing issue', exact: true }).fill('42');
    await page.getByRole('button', { name: 'Start planning', exact: true }).click();
    await expect.poll(() => commands.length).toBe(1);
    assert.equal(commands[0].action, 'link_plan');
    assert.equal(commands[0].id, id);
    assert.equal(commands[0].issue, '42');
    assert.equal(commands[0].text, 'Plan this existing task');
    await expect(page).toHaveURL(`${base}/plans/70`);
    await page.goto(`${base}/jobs/${id}`);
    await page.getByRole('button', { name: 'Plan as issue…' }).click();
    await page.route('**/api/v1/actions', route => route.fulfill({ status: 400, json: { error: 'Issue planning is disabled in demo mode.' } }));
    await page.getByRole('button', { name: 'Start planning', exact: true }).click();
    await expect(page.getByRole('alert')).toHaveText('Issue planning is disabled in demo mode.');
  });
  await test('composer works when localStorage is unavailable', async page => {
    await page.addInitScript(() => { Object.defineProperty(window, 'localStorage', { get() { throw new Error('Storage disabled'); } }); });
    await board(page);
    const input = page.getByRole('textbox', { name: 'New task', exact: true });
    await input.fill('usable without storage');
    await input.press('Control+u');
    await expect(input).toHaveValue('');
  });
  await test('checkbox focus permits global intake and new-task shortcuts', async page => {
    const commands = await fixture(page);
    await board(page);
    await page.getByRole('checkbox', { name: 'Plan first' }).focus();
    await page.keyboard.press('Control+a');
    await expect.poll(() => commands.length).toBe(1);
    assert.equal(commands[0].action, 'automation');
    await page.keyboard.press('n');
    await expect(page.getByRole('textbox', { name: 'New task', exact: true })).toBeFocused();
  });
  await test('submission records the sent text and preserves a newer draft', async page => {
    let release;
    const response = new Promise(resolve => { release = resolve; });
    let started = false;
    await page.route('**/api/v1/actions', async route => {
      started = true;
      await response;
      await route.fulfill({ json: { id: 114 } });
    });
    await board(page);
    const input = page.getByRole('textbox', { name: 'New task', exact: true });
    await input.fill('sent task');
    await input.press('Enter');
    await expect.poll(() => started).toBe(true);
    await input.fill('next draft');
    release();
    await expect(page).toHaveURL(`${base}/jobs/114`);
    await expect(input).toHaveValue('next draft');
    await input.press('ArrowUp');
    await expect(input).toHaveValue('sent task');
    await input.press('ArrowDown');
    await expect(input).toHaveValue('next draft');
  });
} finally { await browser.close(); }
console.log(`${passed} passed; ${failures.length} failed`);
if (failures.length) process.exitCode = 1;
