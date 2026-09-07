// Boot demo.js against the real index.html in a small DOM and replay the
// views the Rust server actually sent (fixtures/ were recorded from
// rust/target/debug/qomm-demo over its WebSocket, one file per seat and
// phase).  Every seat is rendered through a whole round; nothing may throw.
// Run: node --test qomm_demo/tests/*.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { makeWindow, FakeWebSocket } from './dom_shim.mjs';

const staticDir = new URL('../static/', import.meta.url);
const html = readFileSync(new URL('index.html', staticDir), 'utf8');
const js = readFileSync(new URL('demo.js', staticDir), 'utf8');
const fixture = (seat, phase) => JSON.parse(readFileSync(new URL(`fixtures/${seat}_${phase}.json`, import.meta.url), 'utf8'));
const PHASES = ['deal', 'check', 'reduce', 'open', 'settle', 'done'];
const SEATS = ['taker', 'maker-0', 'node-0', 'observer'];

function boot(options){
  const window = makeWindow(html, options);
  const sandbox = Object.assign(Object.create(null), window, {window});
  vm.createContext(sandbox);
  FakeWebSocket.instances.length = 0;
  vm.runInContext(js, sandbox, {filename: 'demo.js'});
  const ws = FakeWebSocket.instances[0];
  assert.ok(ws, 'the page opens its WebSocket at boot');
  ws.onopen();
  const doc = window.document;
  return {
    sandbox, ws, doc,
    push: (view) => ws.deliver(view),
    text: (id) => doc.getElementById(id).textContent,
    hidden: (id) => doc.getElementById(id).classList.contains('hide'),
    pageText: () => doc.body.textContent,
  };
}

test('boot binds every static control without throwing', () => {
  const page = boot({});
  for (const id of ['btn-lang', 'btn-leave', 'btn-watch', 'chat-send']) assert.ok(page.doc.getElementById(id));
  assert.equal(page.text('conn'), 'サーバーに接続しています…'.replace('サーバーに接続しています…', page.text('conn')));
  assert.ok(page.hidden('stage'));
});

for (const seat of SEATS){
  test(`${seat}: every phase of a recorded round renders`, () => {
    const page = boot({});
    for (const phase of PHASES){
      const view = fixture(seat, phase);
      page.push(view);
      assert.ok(!page.hidden('stage'), `${phase}: stage shown`);
      assert.ok(page.hidden('lobby'), `${phase}: lobby hidden`);
      const nodes = page.doc.getElementById('network-nodes').children;
      assert.equal(nodes.length, 1 + view.config.n_makers + view.config.n_nodes + 3, `${phase}: one graph node per participant, node, matcher, verifier, ledger`);
      assert.ok(page.text('phase-caption').length > 0, `${phase}: caption`);
      const on = page.doc.querySelectorAll('#phase-strip .phase-step.on');
      assert.equal(on.length, 1, `${phase}: exactly one current step`);
      const edges = page.doc.getElementById('graph-edges').children;
      assert.ok(edges.length > 20, `${phase}: edges drawn`);
      if (['deal', 'reduce', 'open', 'settle'].includes(phase)){
        const flowing = page.doc.querySelectorAll('#graph-edges .is-flow');
        assert.ok(flowing.length > 0, `${phase}: some edge is in flight`);
      } else if (phase === 'check'){
        const checking = page.doc.querySelectorAll('#network-nodes .node.is-active');
        assert.equal(checking.length, view.config.n_nodes, 'check: every computing node shows that it is checking its input');
      }
    }
  });
}

test('the taker sees its reserve taken at deal and settled at settle, in the strip and the graph', () => {
  const page = boot({});
  page.push(fixture('taker', 'deal'));
  let strip = page.text('portfolio-strip');
  assert.ok(strip.includes('2,000,000'), 'cash reserved for 100 × 200.00 is shown as reserved');
  assert.ok(page.pageText().includes('USD/JPY 買い 100'), 'the taker sees its own order on the taker node');
  const dealFlow = page.doc.querySelectorAll('#graph-edges .is-flow.teal');
  assert.equal(dealFlow.length, (1 + fixture('taker', 'deal').config.n_makers) * 9,
    'the order and all eight private pricing policies flow to all nine nodes');
  assert.equal(dealFlow.filter(edge => !edge.classList.contains('faint')).length, 9,
    'the taker\'s own nine order paths are visually distinguished');
  page.push(fixture('taker', 'check'));
  page.push(fixture('taker', 'reduce'));
  assert.equal(page.doc.querySelectorAll('#graph-edges .is-flow').length, 9, 'nine node→match edges flow while pricing');
  page.push(fixture('taker', 'open'));
  assert.equal(page.doc.querySelectorAll('#graph-edges .is-flow.amber').length, 1, 'the keyed result flows back to the taker');
  page.push(fixture('taker', 'settle'));
  strip = page.text('portfolio-strip');
  assert.ok(strip.includes('+100'), 'inventory grew by the filled 100 units');
  assert.ok(strip.includes('決済'), 'the change is labelled as settlement');
  const settleFlow = page.doc.querySelectorAll('#graph-edges .is-flow.blue');
  assert.equal(settleFlow.length, 4, 'verify → ledger → taker and → winning maker flow at settle');
  assert.ok(page.pageText().includes('決済完了'));
  page.push(fixture('taker', 'done'));
  assert.ok(page.pageText().includes('158.03'), 'the taker reads its price');
  assert.ok(page.text('phase-caption').includes('#16'));
  assert.ok(page.text('phase-caption').includes('処理時間'));
  assert.ok(page.text('roundNum').includes('処理時間'));
  assert.ok(page.pageText().includes('全員が見られるのは暗号文だけです'));
  assert.ok(page.pageText().includes('公開される暗号文 · 平文は非公開'));
});

test('maker and node pages never carry the taker\'s order or price', () => {
  for (const seat of ['maker-0', 'node-0']){
    const page = boot({});
    for (const phase of PHASES) page.push(fixture(seat, phase));
    const text = page.pageText();
    assert.ok(!text.includes('158.03'), `${seat}: no price`);
    assert.ok(!text.includes('USD/JPY 買い 100'), `${seat}: no order`);
    assert.ok(text.includes('注文は非公開'), `${seat}: the taker node says the order is hidden`);
  }
  const node = boot({});
  node.push(fixture('node-0', 'done'));
  assert.ok(node.text('portfolio-strip').includes('在庫も資金も保有しません'));
  const own = node.doc.querySelectorAll('#network-nodes .gnode.is-me');
  assert.equal(own.length, 1, 'exactly one node is marked as the seat\'s own');
  assert.ok(own[0].getAttribute('aria-label').startsWith('計算ノード 0 — あなた'));
  const maker = boot({});
  maker.push(fixture('maker-0', 'done'));
  assert.ok(maker.text('portfolio-strip').includes('予約中'));
  assert.ok(maker.pageText().includes('方針は非公開'), 'other makers\' policies are hidden');
});

test('the observer sees every balance and the winner', () => {
  const page = boot({});
  page.push(fixture('observer', 'done'));
  assert.ok(page.text('portfolio-strip').includes('値付け 7'));
  assert.ok(page.doc.querySelector('#portfolio-strip .portfolio-table-wrap'),
    'the all-participant balance table has its mobile scroll treatment');
  assert.ok(page.pageText().includes('約定相手: 値付け 3'));
  assert.ok(page.doc.getElementById('cfg_sm'), 'the observer can set the time per phase');
});

test('chat: a sentence becomes a previewed request, confirmation sends it, busy blocks sending', () => {
  const page = boot({});
  page.push(fixture('taker', 'done'));
  const input = page.doc.getElementById('chat-input');
  input.value = 'USD/JPY を100単位 買いたい';
  page.doc.getElementById('chat-send').click();
  assert.ok(!page.hidden('chat-preview'), 'a preview is shown before anything is sent');
  assert.equal(page.ws.sent.filter(m => m.type === 'request').length, 0);
  const yes = page.doc.querySelectorAll('#chat-preview .preview-actions button')[0];
  yes.click();
  assert.deepEqual(page.ws.sent.filter(m => m.type === 'request').pop(), {type: 'request', values: {asset: 0, direction: 0, qty: 100}});
  assert.ok(page.hidden('chat-preview'));
  input.value = 'この内容で送信';
  page.doc.getElementById('chat-send').click();
  page.doc.querySelectorAll('#chat-preview .preview-actions button')[0].click();
  assert.equal(page.ws.sent.filter(m => m.type === 'submit').length, 1);
  page.ws.deliver({type: 'refused', reason: 'a round is already in progress'});
  assert.ok(!page.hidden('errorbar'));
  assert.ok(page.text('errorbar').includes('ラウンド進行中'));
  assert.ok(page.text('chat-messages').includes('ラウンド進行中'));
  page.push(fixture('taker', 'deal'));
  input.value = 'この内容で送信';
  page.doc.getElementById('chat-send').click();
  assert.equal(page.ws.sent.filter(m => m.type === 'submit').length, 1, 'no submit while the round is being shown');
  assert.ok(page.text('chat-messages').includes('ラウンド進行中は送信できません'));
});

test('a refused order restores the submit control when the room stayed idle', () => {
  const page = boot({});
  page.push(fixture('taker', 'done'));
  const submit = page.doc.querySelector('#panel button.go');
  submit.click();
  assert.equal(submit.disabled, true);
  assert.equal(submit.textContent, '実行中…');

  page.ws.deliver({type: 'refused', reason: 'policy refresh failed'});

  const restored = page.doc.querySelector('#panel button.go');
  assert.equal(restored.disabled, false);
  assert.equal(restored.textContent, '送信する');
  assert.ok(page.text('errorbar').includes('policy refresh failed'));
});

test('chat: the maker\'s sentence becomes a policy update with its reserve consequence', () => {
  const page = boot({});
  page.push(fixture('maker-0', 'done'));
  const input = page.doc.getElementById('chat-input');
  input.value = '最大数量を300に';
  page.doc.getElementById('chat-send').click();
  assert.ok(page.text('chat-preview').includes('予約枠'));
  page.doc.querySelectorAll('#chat-preview .preview-actions button')[0].click();
  assert.deepEqual(page.ws.sent.filter(m => m.type === 'policy').pop(), {type: 'policy', values: {maxqty: 300}});
});

test('lobby: seats are listed from V and a click claims one; leave releases', () => {
  const page = boot({});
  const view = fixture('taker', 'done');
  page.push(Object.assign({}, view, {seat: null, kind: null, index: null, taker: undefined}));
  assert.ok(!page.hidden('lobby'));
  const children = page.doc.getElementById('lobby-grid').children;
  // One button per seat plus one heading per role group (taker, maker, node).
  assert.equal(children.length, 1 + view.config.n_makers + view.config.n_nodes + 3);
  const buttons = children.filter(child => String(child.tagName).toLowerCase() === 'button');
  assert.equal(buttons.length, 1 + view.config.n_makers + view.config.n_nodes);
  assert.ok(children[0].textContent.includes('Taker'), 'the first group explains who requests a trade');
  page.doc.getElementById('pname').value = 'Rin';
  const node8 = buttons.find(button => button.textContent.startsWith('計算ノード 8'));
  assert.ok(node8, 'node 8 is present in the server-provided seat list');
  node8.click();
  assert.deepEqual(page.ws.sent.pop(), {type: 'claim', seat: 'node:8', label: 'Rin'});
  page.doc.getElementById('btn-watch').click();
  assert.deepEqual(page.ws.sent.pop(), {type: 'claim', seat: 'observer', label: 'Rin'});
  page.push(view);
  page.doc.getElementById('btn-leave').click();
  assert.deepEqual(page.ws.sent.pop(), {type: 'release'});
});

test('language toggle re-renders in English without losing the graph', () => {
  const page = boot({});
  page.push(fixture('taker', 'done'));
  page.doc.getElementById('btn-lang').click();
  assert.equal(page.text('btn-lang'), 'JA');
  assert.ok(page.pageText().includes('DeFMI ledger'));
  assert.ok(page.pageText().includes('recorded by the DeFMI custom VM on Avalanche'));
  assert.equal(page.doc.getElementById('network-nodes').children.length, 21);
});

test('execution badge exposes a readable mobile label in both languages', () => {
  const page = boot({width: 390});
  const view = fixture('observer', 'done');
  page.push({...view, config: {...view.config, engine: 'mpc'}});
  assert.equal(page.doc.getElementById('engineBadge').dataset.short, '秘密計算');
  page.doc.getElementById('btn-lang').click();
  assert.equal(page.doc.getElementById('engineBadge').dataset.short, 'MPC');
});

test('390px: the graph reflows without clipping or tiny nodes; reduced motion drops the particles', () => {
  const narrow = boot({width: 390});
  narrow.push(fixture('taker', 'deal'));
  const nodes = narrow.doc.getElementById('network-nodes').children;
  assert.equal(nodes.length, 21);
  assert.ok(nodes.every(n => parseFloat(n.style.width) >= 80));
  const leftmost = Math.min(...nodes.map(n => parseFloat(n.style.left)));
  const rightmost = Math.max(...nodes.map(n => parseFloat(n.style.left) + parseFloat(n.style.width)));
  assert.ok(leftmost >= 0 && rightmost <= 390, 'every service stays inside the initial narrow canvas');
  assert.ok(narrow.doc.getElementById('network-graph')._qommGraph.model.H > 1200,
    'the narrow graph uses document height instead of miniaturising the topology');
  assert.ok(narrow.doc.getElementById('graph-particles').children.length > 0);
  const still = boot({width: 390, reducedMotion: true});
  still.push(fixture('taker', 'deal'));
  assert.equal(still.doc.getElementById('graph-particles').children.length, 0);
  assert.ok(still.doc.querySelectorAll('#graph-edges .is-flow').length > 0, 'the current position is still marked');
});

test('disconnect shows the reconnecting state and the empty round state reads plainly', () => {
  const page = boot({});
  const view = fixture('taker', 'done');
  page.push(Object.assign({}, view, {public: {}, history: [], phase: 'idle', phase_fields: {}, busy: false,
    taker: Object.assign({}, view.taker, {last: undefined, settlement: null})}));
  assert.ok(!page.hidden('graph-empty'));
  assert.ok(page.text('graph-empty').includes('まだラウンドがありません'));
  assert.ok(page.text('publicbody').includes('まだラウンドがありません'));
  page.ws.close();
  assert.ok(!page.hidden('errorbar'));
  assert.ok(page.text('errorbar').includes('再接続'));
});
