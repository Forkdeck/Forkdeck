// Use the frontend's existing Markdown parser; install with npm ci --prefix web.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';

const require = createRequire(new URL('../web/package.json', import.meta.url));
const MarkdownIt = require('markdown-it');
const markdown = new MarkdownIt({ html: true });
const repository = fileURLToPath(new URL('../', import.meta.url));

function markdownFiles(dir) {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap(entry => {
    const file = path.join(dir, entry.name);
    return entry.isDirectory() ? markdownFiles(file) : /\.md$/i.test(file) ? [file] : [];
  });
}

// GitHub heading IDs retain Unicode letters, spaces become hyphens, punctuation
// disappears, and repeated IDs receive the next unused numeric suffix.
function slug(text) {
  return text.toLowerCase().replace(/[^\p{L}\p{M}\p{N}\p{Pc}\-\s]/gu, '').replace(/ /g, '-');
}

function inlineText(tokens) {
  return (tokens ?? []).map(token => {
    if (token.type === 'html_inline') return '';
    if (token.type === 'image') return inlineText(token.children);
    if (token.type === 'softbreak' || token.type === 'hardbreak') return ' ';
    return token.children ? inlineText(token.children) : token.content;
  }).join('');
}

function parse(file) {
  const tokens = markdown.parse(fs.readFileSync(file, 'utf8'), {});
  const anchors = new Set();
  const headings = new Set();
  const links = [];
  function visit(tokens, line) {
    for (let i = 0; i < tokens.length; i++) {
      const token = tokens[i];
      const at = token.map ? token.map[0] + 1 : line;
      if (token.type === 'heading_open') {
        const base = slug(inlineText(tokens[i + 1]?.children));
        let id = base;
        for (let suffix = 1; headings.has(id); suffix++) id = `${base}-${suffix}`;
        headings.add(id);
        anchors.add(id);
      }
      if (token.type === 'link_open' || token.type === 'image') {
        links.push({ href: token.attrGet(token.type === 'image' ? 'src' : 'href'), line: at });
      }
      if (token.type === 'html_inline' || token.type === 'html_block') {
        // Inspect tags, not comments or prose. HTML links/images and explicit anchors
        // are checked too; code tokens never enter this branch.
        const html = token.content.replace(/<!--[\s\S]*?-->/g, '');
        for (const tag of html.matchAll(/<([a-z][\w-]*)\b([^<>]*)>/gi)) {
          for (const attr of tag[2].matchAll(/\b(id|name|href|src)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+))/gi)) {
            const key = attr[1].toLowerCase();
            const value = markdown.utils.unescapeAll(attr[2] ?? attr[3] ?? attr[4]);
            if (key === 'id' || (key === 'name' && tag[1].toLowerCase() === 'a')) anchors.add(value);
            if (key === 'href' || key === 'src') links.push({ href: value, line: at });
          }
        }
      }
      if (token.children) visit(token.children, at);
    }
  }
  visit(tokens, 1);
  return { anchors, links };
}

export function checkDocs(root = repository) {
  root = path.resolve(root);
  const files = [path.join(root, 'README.md'), ...markdownFiles(path.join(root, 'docs'))];
  const cache = new Map();
  const parsed = file => {
    if (!cache.has(file)) cache.set(file, parse(file));
    return cache.get(file);
  };
  const errors = [];
  let checked = 0;
  for (const file of files) {
    for (const { href, line } of parsed(file).links) {
      // Schemes and protocol-relative URLs are remote, including mailto/data URLs.
      if (/^(?:[a-z][\w+.-]*:|\/\/)/i.test(href)) continue;
      checked++;
      const fail = reason => errors.push(`${path.relative(root, file)}:${line}: ${href}: ${reason}`);
      let pathname, anchor;
      try {
        const hash = href.indexOf('#');
        pathname = decodeURIComponent((hash < 0 ? href : href.slice(0, hash)).split('?')[0]);
        anchor = hash < 0 ? '' : decodeURIComponent(href.slice(hash + 1));
      } catch {
        fail('invalid URL encoding');
        continue;
      }
      let target = pathname ? path.resolve(pathname.startsWith('/') ? root : path.dirname(file), pathname.replace(/^\//, '')) : file;
      if (path.relative(root, target).split(path.sep).includes('..')) {
        fail('target is outside the repository');
        continue;
      }
      if (!fs.existsSync(target)) {
        fail('target does not exist');
        continue;
      }
      if (fs.statSync(target).isDirectory()) {
        // GitHub renders a directory README, if present; plain directory links are valid.
        const readme = path.join(target, 'README.md');
        if (!fs.existsSync(readme)) {
          if (anchor) fail('directory has no README for anchor');
          continue;
        }
        target = readme;
      }
      if (!anchor) continue;
      if (/\.md$/i.test(target)) {
        if (!parsed(target).anchors.has(anchor)) fail(`anchor #${anchor} does not exist`);
      } else {
        // GitHub source views support line and line-range fragments.
        const lines = /^L(\d+)(?:-L(\d+))?$/.exec(anchor);
        const count = fs.readFileSync(target, 'utf8').split('\n').length;
        if (!lines || +lines[1] < 1 || +(lines[2] ?? lines[1]) < +lines[1] || +(lines[2] ?? lines[1]) > count) {
          fail('unsupported or out-of-range file anchor');
        }
      }
    }
  }
  return { errors, files: files.length, checked };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const result = checkDocs();
  if (result.errors.length) {
    console.error(result.errors.join('\n'));
    process.exitCode = 1;
  } else {
    console.log(`Checked ${result.checked} relative links in ${result.files} Markdown files.`);
  }
}
