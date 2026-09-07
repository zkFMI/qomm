// Mechanical agreement between index.html, demo.js and demo.css.
// Run: node --test qomm_demo/tests/*.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';

const staticDir = new URL('../static/', import.meta.url);
const html = readFileSync(new URL('index.html', staticDir), 'utf8');
const js = readFileSync(new URL('demo.js', staticDir), 'utf8');
const css = readFileSync(new URL('demo.css', staticDir), 'utf8');
const flowCss = readFileSync(new URL('react-flow.css', staticDir), 'utf8');
const require = createRequire(import.meta.url);
const demo = require('../static/demo.js');

const htmlIds = new Set([...html.matchAll(/\sid="([^"]+)"/g)].map(m => m[1]));
const jsIdRefs = new Set([...js.matchAll(/\$\('([^']+)'\)/g)].map(m => m[1]));
const dataS = new Set([...html.matchAll(/data-s(?:-placeholder|-aria)?="([^"]+)"/g)].map(m => m[1]));

test('every id the script looks up exists in the page', () => {
  const missing = [...jsIdRefs].filter(id => !htmlIds.has(id));
  assert.deepEqual(missing, []);
});

test('the self-hosted React Flow bundle and stylesheet are loaded before the controller', () => {
  assert.ok(html.includes('src="/react-flow.js'));
  assert.ok(html.includes('href="/react-flow.css'));
  assert.ok(html.indexOf('src="/react-flow.js') < html.indexOf('src="/demo.js'));
  assert.ok(flowCss.includes('.react-flow'));
  assert.ok(js.includes('window.QommNetworkGraph.render'));
});

test('every data-s key in the page has a string in both languages', () => {
  for (const key of dataS) {
    assert.equal(typeof demo.S.ja[key], 'string', `ja ${key}`);
    assert.equal(typeof demo.S.en[key], 'string', `en ${key}`);
  }
});

test('every static class the script assigns has a rule in the stylesheet', () => {
  const tokens = new Set();
  const grab = (re) => { for (const m of js.matchAll(re)) m[1].split(/\s+/).forEach(x => x && tokens.add(x)); };
  grab(/\bel\('[a-z]+',\s*'([^']+)'/g);
  grab(/classList\.(?:add|toggle)\('([^']+)'/g);
  grab(/classes\.push\('([^']+)'\)/g);
  grab(/cls \+= ' ([a-z-]+)'/g);
  grab(/svgEl\('[a-z]+',\s*\{[^}]*class:\s*'([^']+)'/g);
  // Graph card classes (is-me, is-slim, …) are styled by the React Flow
  // bundle's stylesheet; page classes by demo.css. Either sheet counts.
  const missing = [...tokens].filter(c => !css.includes('.' + c) && !flowCss.includes('.' + c));
  assert.deepEqual(missing, []);
});

test('the stylesheet does not keep selectors for elements the page no longer has', () => {
  for (const stale of ['.phases', '.phase .n', '.phase-flow', '.graph-node', '.lobby-controls', '#operator', '.flow-particle']) {
    assert.ok(!css.includes(stale), stale);
  }
});

test('the main screen uses plain Japanese and states the demo/product boundary, not EVM', () => {
  const jargon = ['RFQ', '持ち分', '次数低減', '覆い', '原子的', '神の視点', '嘘'];
  const walk = (value, path) => {
    if (typeof value === 'string') {
      for (const word of jargon) assert.ok(!value.includes(word), `${path} contains ${word}`);
    } else if (Array.isArray(value)) value.forEach((v, i) => walk(v, `${path}[${i}]`));
  };
  for (const [key, value] of Object.entries(demo.S.ja)) walk(value, key);
  for (const word of jargon) assert.ok(!html.includes(word), `index.html contains ${word}`);
  for (const text of [html, js, css]) assert.ok(!/\bEVM\b/.test(text));
  assert.ok(demo.S.ja.productBoundary.includes('Avalanche') && demo.S.ja.productBoundary.includes('DeFMI'));
  assert.ok(demo.S.ja.productBoundary.includes('zkPI') && demo.S.ja.productBoundary.includes('DvP'));
  assert.equal(demo.S.ja.gLedger, 'DeFMI 台帳');
  assert.equal(demo.S.ja.gNode, '計算ノード');
});

test('the page declares the accessibility hooks the script relies on', () => {
  assert.ok(/<h1\b[^>]*data-s="title"/.test(html));
  assert.ok(/id="lobby-grid"[^>]*role="group"/.test(html));
  assert.ok(!/b\.setAttribute\('role',\s*'listitem'\)/.test(js));
  assert.ok(/id="errorbar"[^>]*role="alert"[^>]*aria-live="assertive"/.test(html));
  assert.ok(/id="phase-caption"[^>]*aria-live="polite"/.test(html));
  assert.ok(/id="chat-messages"[^>]*role="log"/.test(html));
  assert.ok(/prefers-reduced-motion/.test(css));
  assert.ok(/focus-visible/.test(css));
  assert.ok(/max-width:430px/.test(css));
});
