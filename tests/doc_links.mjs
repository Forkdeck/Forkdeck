import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { checkDocs } from '../scripts/check-doc-links.mjs';

function fixture(t, files) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'forkdeck-doc-links-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  fs.mkdirSync(path.join(root, 'docs'), { recursive: true });
  for (const [name, content] of Object.entries(files)) {
    const file = path.join(root, name);
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, content);
  }
  return root;
}

test('checks nested docs, reference links, images, encoded paths and heading IDs', t => {
  const root = fixture(t, {
    'README.md': `# Front
[Guide][guide]

[guide]: docs/guide.md#hello-world

![Capture](docs/a%20capture.png)
[Directory](docs/nested/#child)
[Root](/docs/guide.md#hello-world-2)
[Query](docs/guide.md?raw=1#hello-world-1)
[Unicode](docs/guide.md#über-café)
[Code](src/main.rs#L1-L2)
`,
    'docs/guide.md': `# Hello *world*!
# Hello world
# Hello world-1
# Hello world
# Über café
[Self](#hello-world-1)
[Parent](../README.md#front)
`,
    'docs/nested/README.md': 'Child\n=====\n',
    'docs/a capture.png': '',
    'src/main.rs': 'fn main() {\n}\n',
  });
  const result = checkDocs(root);
  assert.deepEqual(result.errors, []);
  assert.equal(result.files, 3);
  assert.equal(result.checked, 9);
});

test('reports each missing file and anchor with its source', t => {
  const root = fixture(t, {
    'README.md': '[Missing](docs/no.md)\n[Bad anchor](docs/guide.md#absent)\n![Missing image](docs/no.png)\n',
    'docs/guide.md': '[Nested](nested/missing.md)\n[Self](#no)\n# Guide\n',
  });
  const result = checkDocs(root);
  assert.equal(result.errors.length, 5);
  assert.match(result.errors[0], /README.md:1: docs\/no.md: target does not exist/);
  assert.match(result.errors[1], /anchor #absent does not exist/);
  assert.match(result.errors[3], /docs\/guide.md:1: nested\/missing.md/);
});

test('ignores code, comments and external URLs while checking HTML anchors and links', t => {
  const root = fixture(t, {
    'README.md': `# Front
\`[inline](missing.md)\`
\`\`\`md
[fenced](missing.md)
# This is not a heading
\`\`\`
<!-- <a href="missing.md">comment</a> -->
[External](https://example.invalid/404)
[Mail](mailto:docs@example.invalid)
[CDN](//example.invalid/file)
<a id="custom"></a>
<a name='legacy'></a>
<a href='#custom'>Custom</a>
[Legacy](#legacy)
<img src="docs/capture.png">
[False heading](#this-is-not-a-heading)
`,
    'docs/capture.png': '',
  });
  const result = checkDocs(root);
  assert.equal(result.checked, 4);
  assert.equal(result.errors.length, 1);
  assert.match(result.errors[0], /anchor #this-is-not-a-heading does not exist/);
});

test('rejects escaping paths, bad URL encoding and invalid file fragments', t => {
  const root = fixture(t, {
    'README.md': '[Escape](../README.md)\n[Encoding](docs/%FF.md)\n[Source](source.rs#L99)\n[Reversed](source.rs#L2-L1)\n[Directory](docs/empty/#missing)\n',
    'source.rs': 'fn main() {}\n',
  });
  fs.mkdirSync(path.join(root, 'docs/empty'));
  const result = checkDocs(root);
  assert.equal(result.errors.length, 5);
  assert.match(result.errors[0], /outside the repository/);
  assert.match(result.errors[1], /invalid URL encoding/);
  assert.match(result.errors[2], /out-of-range file anchor/);
  assert.match(result.errors[4], /directory has no README/);
});

test('repository documentation has no broken relative links or anchors', () => {
  const result = checkDocs();
  assert.deepEqual(result.errors, []);
});
