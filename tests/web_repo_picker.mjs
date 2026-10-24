// Browser-only forge fixtures follow Tasks 2 and 3's response contracts.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
const require = createRequire(new URL('../web/package.json', import.meta.url));
const { expect } = require('@playwright/test');

export async function checkRepoPicker(browser, base, width) {
  const page = await browser.newPage({viewport:{width,height:900}});
  page.setDefaultNavigationTimeout(30000);
  const errors = [];
  page.on('pageerror', error => errors.push(String(error)));
  const providers = [{id:'github',host:'github.com'},{id:'gitlab',host:'gitlab.example.test'}];
  let canAdd = true;
  let expired = false;
  let pickerStatus = 200;
  let actionStatus = 502;
  const requests = [];
  const picker = {
    provider:'gitlab', host:'gitlab.example.test', expires_unix:Math.floor(Date.now()/1000)+600,
    repos:[
      {repo:'team/platform/service',private:true,default_branch:'trunk',description:'Deployment orchestration',configured:false},
      {repo:'team/already',private:false,default_branch:'main',description:'Existing repository',configured:true},
      {repo:'team/another',private:false,default_branch:'develop',description:'Public tools <script>throw new Error("unsafe")</script>',configured:false},
    ],
  };
  await page.route('**/api/v1/forges', route => route.fulfill({json:{providers,can_add:canAdd}}));
  await page.route('**/api/v1/forges/pickers/*', route => route.fulfill({
    status:expired ? 404 : pickerStatus,
    json:expired ? {error:'Not found'} : pickerStatus === 200 ? picker : {error:'Picker unavailable'},
  }));
  await page.route('**/api/v1/actions', async route => {
    const request = route.request();
    requests.push({body:request.postDataJSON(),key:request.headers()['idempotency-key'],origin:request.headers().origin});
    await route.fulfill({status:actionStatus,json:actionStatus === 200
      ? {repo:'team/platform/service',path:'/srv/repos/gitlab.example.test/team/platform/service',restart_required:true}
      : {error:'Could not resolve to a Repository'}});
  });
  try {
    await page.goto(`${base}/repos`);
    for (const [id, label] of [['github','GitHub'],['gitlab','GitLab']]) {
      await expect(page.getByRole('link',{name:`Add from ${label}`,exact:true})).toHaveAttribute('href',`/forges/${id}/connect`);
    }
    // Connect must be a document navigation, never client-side routing or a fetch.
    await page.route('**/forges/github/connect', async route => {
      assert.equal(route.request().isNavigationRequest(),true);
      await route.fulfill({contentType:'text/html',body:'<h1>Forge authorization</h1>'});
    });
    await page.getByRole('link',{name:'Add from GitHub',exact:true}).click();
    await expect(page.getByRole('heading',{name:'Forge authorization'})).toBeVisible();
    canAdd = false;
    await page.goto(`${base}/repos`);
    await expect(page.getByRole('button',{name:'Add from GitHub',exact:true})).toBeDisabled();
    await expect(page.getByText('Operator or admin role required', {exact:true})).toBeVisible();
    assert.equal(await page.locator('a[href^="/forges/"]').count(),0);
    await page.goto(`${base}/repos/add/test-picker`);
    await expect(page.getByRole('button',{name:'Add',exact:true}).first()).toBeDisabled();
    canAdd = true;
    await page.goto(`${base}/repos/add/test-picker`);
    await expect(page.getByRole('heading',{name:'Add a repository from gitlab.example.test'})).toBeVisible();
    const service = page.getByRole('listitem').filter({has:page.getByRole('heading',{name:'team/platform/service',exact:true})});
    await expect(service.getByText('Private',{exact:true})).toBeVisible();
    await expect(service.getByText('trunk',{exact:true})).toBeVisible();
    const already = page.getByRole('listitem').filter({has:page.getByRole('heading',{name:'team/already',exact:true})});
    await expect(already.getByText('Public',{exact:true})).toBeVisible();
    await expect(already.getByText('Already added',{exact:true})).toBeVisible();
    assert.equal(await already.getByRole('button',{name:'Add',exact:true}).count(),0);
    const search = page.getByRole('searchbox',{name:'Search repositories'});
    await search.fill('ORCHESTRATION');
    await expect(page.getByRole('listitem')).toHaveCount(1);
    await search.fill('TEAM/ALREADY');
    await expect(page.getByRole('listitem')).toHaveCount(1);
    await search.fill('no matching repo');
    await expect(page.getByText('No repositories match your search.',{exact:true})).toBeVisible();
    await search.fill('');
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false,`picker overflow at ${width}`);
    await page.screenshot({path:`/tmp/forkdeck-picker-${width}.png`,fullPage:false});
    await service.getByRole('button',{name:'Add',exact:true}).click();
    await expect(page.getByRole('alert')).toHaveText('Could not resolve to a Repository');
    assert.deepEqual(requests[0].body,{action:'add_repo',picker:'test-picker',repo:'team/platform/service'});
    assert.match(requests[0].key,/^[0-9a-f]{32}$/);
    assert.equal(requests[0].origin,new URL(base).origin);
    actionStatus = 200;
    await service.getByRole('button',{name:'Add',exact:true}).click();
    await expect(page.getByRole('status')).toContainText('/srv/repos/gitlab.example.test/team/platform/service');
    await expect(page.getByRole('status')).toContainText('Restart Forkdeck to start watching it.');
    await expect(service.getByText('Already added',{exact:true})).toBeVisible();
    assert.notEqual(requests[0].key,requests[1].key);
    expired = true;
    await page.goto(`${base}/repos/add/expired-picker`);
    await expect(page.getByText('This list expired. Connect again.',{exact:true})).toBeVisible();
    await expect(page.getByRole('link',{name:'Add from GitLab',exact:true})).toHaveAttribute('href','/forges/gitlab/connect');
    expired = false;
    pickerStatus = 503;
    await page.goto(`${base}/repos/add/unavailable-picker`);
    await expect(page.getByRole('alert')).toHaveText('Picker unavailable');
    assert.equal(await page.getByText('This list expired. Connect again.',{exact:true}).count(),0);
    assert.deepEqual(errors,[]);
    console.log(`Repository picker fixtures at ${width}px: passed`);
  } finally { await page.close(); }
}
