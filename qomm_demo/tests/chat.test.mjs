// The chat turns sentences into the same request/policy messages the panel
// sends, deterministically.  Run: node --test qomm_demo/tests/*.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const demo = require('../static/demo.js');

const assets = [
  { name: 'USD/JPY', reference: 15_750, scale: 100 },
  { name: 'EUR/USD', reference: 10_850, scale: 10_000 },
  { name: 'BTC/USD', reference: 6_420_000, scale: 100 },
];
const takerView = {
  assets,
  taker: { pending: { asset: 0, qty: 100, direction: 0, is_real: 1, limit_price: 15_900 } },
};
const policy = { asset: 0, ask_level: 0, spread: 40, slope: 1, invcoef: 1, inv: 0,
  maxqty: 200, expiry: 1_000_000_000, active: 1, use_ref: 1 };
const makerView = { assets, maker: { policy, reserve: { asset: 0, inventory: 200, cash: 3_102_000 } } };

const only = (result, type) => result.actions.filter(a => a.msg.type === type);

test('elapsed time is shown as a human duration instead of an unexplained millisecond count', () => {
  assert.equal(demo.formatDuration(307.9, 'ja'), '0.31秒');
  assert.equal(demo.formatDuration(12_871, 'ja'), '12.9秒');
  assert.equal(demo.formatDuration(91_317.3, 'ja'), '1分31.3秒');
  assert.equal(demo.formatDuration(91_317.3, 'en'), '1 min 31.3 s');
});

test('taker: market, side and size in one Japanese sentence', () => {
  const r = demo.interpretTaker('USD/JPY を100単位 買いたい', takerView);
  assert.deepEqual(only(r, 'request')[0].msg, { type: 'request', values: { asset: 0, direction: 0, qty: 100 } });
  assert.equal(only(r, 'submit').length, 0);
  assert.deepEqual(r.warnings, []);
});

test('taker: a limit is scaled into the ticks the server expects', () => {
  const r = demo.interpretTaker('上限を 158.5 にして', takerView);
  assert.deepEqual(only(r, 'request')[0].msg.values, { limit_price: 15_850 });
  const eur = demo.interpretTaker('EUR/USD の上限を 1.09 に', takerView);
  assert.deepEqual(eur.actions[0].msg.values, { asset: 1, limit_price: 10_900 });
});

test('taker: dummy traffic and live orders', () => {
  assert.deepEqual(demo.interpretTaker('ダミー通信に切り替え', takerView).actions[0].msg.values, { is_real: 0 });
  assert.deepEqual(demo.interpretTaker('実注文に戻して', takerView).actions[0].msg.values, { is_real: 1 });
});

test('taker: send alone becomes a submit that states the reserve it will take', () => {
  const r = demo.interpretTaker('この内容で送信', takerView);
  assert.deepEqual(r.actions.map(a => a.msg), [{ type: 'submit' }]);
  assert.ok(r.actions[0].desc.some(line => line.includes('1,590,000')), r.actions[0].desc.join('|'));
});

test('taker: settings followed by a send keep their order and describe the inventory reserve', () => {
  const r = demo.interpretTaker('EUR/USD を 50 売って発注', takerView);
  assert.deepEqual(r.actions.map(a => a.msg), [
    { type: 'request', values: { asset: 1, direction: 1, qty: 50 } },
    { type: 'submit' },
  ]);
  assert.ok(r.actions[1].desc.some(line => line.includes('50') && line.includes('EUR/USD')));
});

test('taker: contradictions and out-of-range sizes are reported, nonsense is refused', () => {
  const both = demo.interpretTaker('買いも売りも 10単位', takerView);
  assert.equal(both.actions[0].msg.values.direction, undefined);
  assert.equal(both.warnings.length, 1);
  const big = demo.interpretTaker('数量を 900 に', takerView);
  assert.equal(big.actions[0].msg.values.qty, 500);
  assert.equal(big.warnings.length, 1);
  assert.equal(demo.interpretTaker('こんにちは', takerView), null);
});

test('taker: English works the same way', () => {
  demo.setLang('en');
  const r = demo.interpretTaker('buy 100 units of USD/JPY and send', takerView);
  assert.deepEqual(r.actions.map(a => a.msg), [
    { type: 'request', values: { asset: 0, direction: 0, qty: 100 } },
    { type: 'submit' },
  ]);
  demo.setLang('ja');
});

test('maker: absolute and relative policy changes', () => {
  assert.deepEqual(demo.interpretMaker('スプレッドを30に', makerView).actions[0].msg, { type: 'policy', values: { spread: 30 } });
  assert.deepEqual(demo.interpretMaker('スプレッドを10広げて', makerView).actions[0].msg.values, { spread: 50 });
  assert.deepEqual(demo.interpretMaker('スプレッドを10狭めて', makerView).actions[0].msg.values, { spread: 30 });
  assert.deepEqual(demo.interpretMaker('基準からのずれを -5 に', makerView).actions[0].msg.values, { ask_level: -5 });
  assert.deepEqual(demo.interpretMaker('spread 30', makerView).actions[0].msg.values, { spread: 30 });
  assert.deepEqual(demo.interpretMaker('max size 300', makerView).actions[0].msg.values, { maxqty: 300 });
});

test('maker: the reserve consequence of max size and on/off is stated with the server formula', () => {
  assert.equal(demo.makerCashRequirement(policy, assets), 200 * (15_750 - 40 - 200));
  const bigger = demo.interpretMaker('最大数量を300に', makerView);
  assert.deepEqual(bigger.actions[0].msg.values, { maxqty: 300 });
  const expected = demo.makerCashRequirement({ ...policy, maxqty: 300 }, assets);
  assert.ok(bigger.actions[0].desc.some(line => line.includes('300') && line.includes(expected.toLocaleString())), bigger.actions[0].desc.join('|'));
  const off = demo.interpretMaker('一時停止', makerView);
  assert.deepEqual(off.actions[0].msg.values, { active: 0 });
  assert.ok(off.actions[0].desc.some(line => line.includes('0') && line.includes('200')));
  assert.deepEqual(demo.interpretMaker('再開', makerView).actions[0].msg.values, { active: 1 });
  assert.deepEqual(demo.interpretMaker('switch off', makerView).actions[0].msg.values, { active: 0 });
});

test('maker: market change and range clamping', () => {
  assert.deepEqual(demo.interpretMaker('対象を EUR/USD に', makerView).actions[0].msg.values, { asset: 1 });
  const clamped = demo.interpretMaker('最大数量を 9000 に', makerView);
  assert.equal(clamped.actions[0].msg.values.maxqty, 500);
  assert.equal(clamped.warnings.length, 1);
  assert.equal(demo.interpretMaker('なにもしない', makerView), null);
});

test('server refusals are translated with their numbers intact', () => {
  const cash = demo.translateRefusal('Taker has 5 cash units available but the signed limit needs 1000');
  assert.ok(cash.includes('5') && cash.includes('1,000'));
  assert.ok(demo.translateRefusal('a round is already in progress') !== 'a round is already in progress');
  assert.equal(demo.translateRefusal('something else'), 'something else');
});

test('orthogonal edge paths start and end at their endpoints and drop duplicate points', () => {
  const d = demo.orthoPath([{ x: 0, y: 0 }, { x: 0, y: 0 }, { x: 0, y: 40 }, { x: 60, y: 40 }, { x: 60, y: 80 }]);
  assert.ok(d.startsWith('M0,0'));
  assert.ok(d.endsWith('L60,80'));
  assert.equal((d.match(/Q/g) || []).length, 2);
});
