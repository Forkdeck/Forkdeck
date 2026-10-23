// Build web/ and the Rust binary first. This starts a temporary demo server and captures
// both the app and the approved Design Component reference with the same local fonts.
import { chromium } from '@playwright/test';
import fs from 'node:fs';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
const root = path.resolve(import.meta.dirname, '../..');
const server = spawn(process.env.FORKDECK_BIN || path.join(root, 'target/debug/forkdeck'), ['serve','--demo'], {cwd:root,stdio:['ignore','pipe','pipe']});
let output = '';
server.stderr.on('data', data => process.stderr.write(data));
const login = await new Promise((resolve,reject) => {
  const timeout = setTimeout(() => reject(new Error('Server did not print a login URL')), 20000);
  server.once('exit', code => reject(new Error(`Server exited ${code}`)));
  server.stdout.on('data', data => {
    output += data;
    const url = output.match(/http:\/\/127\.0\.0\.1:\d+\/auth\/login\?code=[a-f0-9]+/);
    if (url) {clearTimeout(timeout);resolve(url[0]);}
  });
});
const browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH || '/home/justin/.cache/ms-playwright/chromium-1187/chrome-linux/chrome', headless:true });
try {
  const page = await browser.newPage({viewport:{width:1440,height:900}});
  const errors=[];
  page.on('pageerror', e => errors.push(String(e)));
  page.on('response',r => { if(r.url().includes('/auth'))console.log(r.status(),new URL(r.url()).pathname); });
  await page.goto(login);
  await page.waitForURL('http://127.0.0.1:7420/',{timeout:10000}).catch(async e => {console.log('Login body:',await page.locator('body').innerText(), errors); throw e;});
  for (const [name,url,width,height,file] of [
    ['Main','/jobs/72',1440,900,'board-1440x900.png'],
    ['Plan','/plans',1440,900,'plans-1440x900.png'],
    ['Phone','/',390,844,'board-390x844.png'],
  ]) {
    await page.setViewportSize({width,height});
    await page.goto('http://127.0.0.1:7420'+url);
    await page.getByRole(name==='Phone' ? 'tab' : 'heading', {name:name==='Main' ? 'semantic-context#22 Home record API' : name==='Plan' ? 'Home records and sync' : 'Active',exact:true}).waitFor();
    await page.evaluate(() => document.fonts.ready);
    if(name !== 'Plan') await page.getByText('Live',{exact:name==='Phone'}).first().waitFor();
    await page.screenshot({path:path.join(root,'docs/web-ui-mockup/screenshots',file)});
    const geometry=()=>[...document.querySelectorAll('span,div,a')].filter(e=>e.childNodes.length && [...e.childNodes].every(n=>n.nodeType===3)).map(e=>{const r=e.getBoundingClientRect(),s=getComputedStyle(e);return {text:e.textContent,rect:[r.x,r.y,r.width,r.height],font:s.font,fontKerning:s.fontKerning,letterSpacing:s.letterSpacing}});
    fs.writeFileSync(`/tmp/forkdeck-geometry-${name}-app.json`,JSON.stringify(await page.evaluate(geometry),null,2));
    const source=fs.readFileSync(path.join(root,`docs/web-ui-mockup/${name}.dc.html`),'utf8');
    const css=fs.readFileSync(path.join(root,'web/dist/assets',fs.readdirSync(path.join(root,'web/dist/assets')).find(f=>f.endsWith('.css'))),'utf8');
    await page.evaluate(({source,css}) => {
      const parsed=new DOMParser().parseFromString(source,'text/html');
      const logic=parsed.querySelector('script[type="text/x-dc"]').textContent;
      class DCLogic { setState() {} }
      const values=new Function('DCLogic',logic+';return new Component({}).renderVals();')(DCLogic);
      const template=parsed.querySelector('x-dc');
      template.querySelector('helmet').remove();
      const value=(text,ctx) => text.trim().split('.').reduce((v,key)=>v?.[key],ctx);
      const binding=(text,ctx)=>text.replace(/\{\{\s*(.*?)\s*\}\}/g,(_,expr)=>value(expr,ctx) ?? '');
      function walk(node,ctx) {
        if(node.nodeType===3) {node.textContent=binding(node.textContent,ctx);return;}
        if(node.nodeType!==1)return;
        if(node.tagName==='SC-FOR') {
          const list=value(node.getAttribute('list').slice(2,-2),ctx);
          for(const item of list) for(const child of [...node.childNodes]) {
            const clone=child.cloneNode(true);node.before(clone);walk(clone,{...ctx,[node.getAttribute('as')]:item});
          }
          node.remove();return;
        }
        if(node.tagName==='SC-IF') {
          if(value(node.getAttribute('value').slice(2,-2),ctx)) {
            for(const child of [...node.childNodes]) {node.before(child);walk(child,ctx);}
          }
          node.remove();return;
        }
        for(const attr of [...node.attributes]) {
          if(attr.name.toLowerCase().startsWith('on'))node.removeAttribute(attr.name);
          else node.setAttribute(attr.name,binding(attr.value,ctx));
        }
        for(const child of [...node.childNodes])walk(child,ctx);
      }
      for(const node of [...template.childNodes])walk(node,values);
      const faces=[...css.matchAll(/@font-face\{[^}]+\}/g)].map(m=>m[0]).join('');
      document.head.innerHTML='<style>body{margin:0;background:#0F1115}a{color:#F2A541}'+faces+'</style>';
      document.body.innerHTML=template.innerHTML;
    },{source,css});
    await page.evaluate(() => document.fonts.ready);
    await page.screenshot({path:`/tmp/forkdeck-reference-${name}.png`});
    fs.writeFileSync(`/tmp/forkdeck-geometry-${name}-reference.json`,JSON.stringify(await page.evaluate(geometry),null,2));
  }
  // Exercise read-only interactions after capturing the approved initial states.
  await page.goto('http://127.0.0.1:7420/');
  await page.getByRole('tab',{name:'Attention',exact:true}).click();
  await page.getByText('nothing built or tested',{exact:true}).waitFor();
  await page.getByText('Plans render a readable task list (first try)',{exact:true}).click();
  await page.getByText('Not built or tested:',{exact:false}).waitFor();
  await page.getByRole('tab',{name:'Reviewer',exact:true}).click();
  await page.getByText('codex: PASS (gate results remain unverified)',{exact:false}).waitFor();
  await page.setViewportSize({width:1440,height:900});
  await page.goto('http://127.0.0.1:7420/plans');
  await page.getByRole('tab',{name:'Plan',exact:true}).click();
  await page.getByRole('heading',{name:'Goal',exact:true}).waitFor();
  await page.getByRole('tab',{name:'Review',exact:true}).click();
  await page.getByText('VERDICT: PASS',{exact:true}).waitFor();
  if(errors.length)throw new Error(errors.join('\n'));
  console.log('Captured board, plans and phone; reference renders saved under /tmp/forkdeck-reference-*.png');
} finally {
  await browser.close();
  server.kill('SIGINT');
  await once(server,'exit');
}
