// Node 24 strips TypeScript types; run: node tests/web_markdown.mjs
import assert from 'node:assert/strict';
import { renderMarkdown } from '../web/src/render-markdown.ts';
const output = renderMarkdown('# Heading\n\n- first\n- second\n\n```js\nalert("code")\n```\n\n[good](https://example.org) [http](http://example.org) [bad](javascript:alert(1)) [data](data:text/html,bad) [relative](/settings)\n\n<script>alert(1)</script><img src=x onerror=alert(1)>\n\n![tracking](https://example.org/pixel)');
assert.match(output, /<h1>Heading<\/h1>/);
assert.match(output, /<ul>/);
assert.match(output, /<pre><code/);
assert.match(output, /href="https:\/\/example.org"/);
assert.match(output, /href="http:\/\/example.org"/);
assert.doesNotMatch(output, /<(script|img)|href="(javascript:|data:|\/settings)/);
assert.match(output, /&lt;script&gt;/);
for (const scheme of ['javascript:alert%281%29', 'file:///etc/passwd', 'mailto:a@b.c', '//example.org', 'HTTPS://example.org', 'https://example.org/?a=1&b=2']) {
  const html = renderMarkdown(`[link](${scheme})`);
  assert.equal(html.includes('<a '), /^https?:/i.test(scheme), scheme);
}
console.log('Markdown safety and supported structures: passed');
