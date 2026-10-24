// Run against a seeded server: FORKDECK_WEB_URL=http://127.0.0.1:17420 node tests/web_pages.mjs
import assert from 'node:assert/strict';
import { checkRepoPicker } from './web_repo_picker.mjs';
import { createRequire } from 'node:module';
const require = createRequire(new URL('../web/package.json', import.meta.url));
const { chromium } = require('@playwright/test');
const base = process.env.FORKDECK_WEB_URL || 'http://127.0.0.1:17420';
const browser = await chromium.launch({headless:true,executablePath:process.env.FORKDECK_CHROMIUM});
try {
  for (const width of [1440,390]) {
    const page = await browser.newPage({viewport:{width,height:900}});
    assert.equal((await page.request.get(`${base}/api/v1/forges`)).status(),404);
    const errors=[];
    page.on('pageerror',error=>errors.push(String(error)));
    for (const path of ['repos','deploys','stats','doctor']) {
      const r=await page.goto(`${base}/${path}`);
      assert.equal(r.status(),200);
      await page.waitForFunction(()=>!document.querySelector('[role=status]'));
      await page.locator('.operation-panel').first().waitFor();
      assert.ok(await page.locator('main h1').innerText());
      if (path === 'repos') assert.equal(await page.locator('[href^="/forges/"]').count(),0);
      assert.equal(await page.locator('[role=alert]').count(),0);
      assert.ok(await page.locator('nav a[href="/plans"]').count());
      const overflow=await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth);
      assert.equal(overflow,false,`${path} overflow at ${width}`);
      await page.screenshot({path:`/tmp/forkdeck-${path}-${width}.png`,fullPage:false});
    }
    const plans=await (await page.request.get(`${base}/api/v1/plans`)).json();
    const selected=plans.find(p=>p.plan.graph);
    assert.ok(selected);
    await page.goto(`${base}/plans/${selected.id}`);
    await page.getByRole('tab',{name:'Plan',exact:true}).click();
    await page.locator('.markdown h1').waitFor();
    assert.equal(await page.locator('.markdown').innerText().then(t=>t.includes('forkdeck-tasks')),false);
    assert.ok(await page.locator('.markdown pre code').count());
    assert.ok(await page.locator('.markdown a[href^="https://"]').count());
    await page.getByRole('tab',{name:'Review',exact:true}).click();
    assert.match(await page.locator('.plan-prose .task-pill').innerText(),/PASS|FAIL/);
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false,`plans overflow at ${width}`);
    await page.screenshot({path:`/tmp/forkdeck-plan-${width}.png`,fullPage:false});
    const requests=[], sockets=[], snapshots=[];
    page.on('request', request=>requests.push(request.url()));
    page.on('websocket', socket=>sockets.push(socket.url()));
    page.on('response', response=>{
      if (new URL(response.url()).pathname === '/api/v1/snapshot') snapshots.push(response);
    });
    await page.goto(base);
    await page.locator(`nav a[href="/repos"]`).first().waitFor();
    await page.waitForFunction(()=>[...document.querySelectorAll('.connection,.phone-connection')].some(el=>el.textContent.includes('Live')));
    assert.equal(requests.some(url=>new URL(url).pathname === '/api/v1/events'),false);
    assert.ok(sockets.some(url=>new URL(url).pathname === '/api/v1/live'));
    assert.ok(snapshots.length);
    for (const response of snapshots) {
      assert.equal(response.status(),200);
      assert.equal((await response.allHeaders())['content-type'],'application/octet-stream');
    }
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false,`board overflow at ${width}`);
    assert.deepEqual(errors,[]);
    await page.close();
    console.log(`Operational pages and plans at ${width}px: passed`);
    await checkRepoPicker(browser, base, width);
  }
} finally {await browser.close();}
