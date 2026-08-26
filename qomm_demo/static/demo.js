/* One page, one seat. What the page can draw is what the server sent it, and
   the server sends each seat only its own view --- so there is nothing here to
   hide, and nothing that opening the console would reveal. */
'use strict';

const S = {
ja: {
  title:"QOMM", subtitle:"注文を見せずに最良気配を出す",
  leave:"席を離れる", langOther:"EN",
  lobbyTitle:"席を選ぶ",
  lobbyWhy:"席ごとに見えるものが違います。それがこのデモの主張そのものです。誰も座っていない席は自動で動きます。",
  yourName:"あなたの名前（任意）", watch:"観る（神の視点）",
  theSeats:"空席は自動、名前が出ている席は人が操作中",
  whatIsThis:"何を見せているか", publicTitle:"公開されたもの",
  publicWhy:"ラウンドが終わったあと、部屋の全員が見られるもの。値段は入っていません。",
  seatsTitle:"席", seatsWhy:"自動か手動か。ノードが何をするつもりかは、本人以外には見えません。",
  noticesTitle:"この席への通知",
  footer:"QOMM demo — ",
  taken:"使用中", auto:"自動", manual:"手動",
  phaseDeal:"配る", phaseCheck:"検査", phaseReduce:"次数低減", phaseOpen:"開示",
  phaseIdle:"待機", phaseDone:"完了",
  // taker
  takerTitle:"注文（あなただけが見ています）",
  takerWhy:"送るのは持ち分だけです。どのノードも、この画面の中身を組み立てられません。",
  asset:"銘柄", side:"売買", buy:"買い", sell:"売り", qty:"数量",
  kind:"種別", real:"本物", cover:"カバー",
  coverWhy:"カバーは同じ回路・同じバイト数・同じ時間で走ります。外から見て区別がつきません。",
  submit:"送信する", waiting:"実行中…",
  openedTitle:"開示された値",
  openedWhy:"部屋の全員がこの数字を見ています。覆いを持っているのはあなただけです。",
  minusMask:"あなたの覆いを引く",
  yourPrice:"あなたの値段", winnerIs:"勝ったメイカー",
  noMaker:"条件に合うメイカーがいませんでした",
  announce:"勝者に知らせる", announced:"通知済み",
  announceWhy:"メイカーは自分が勝ったことを知りません。あなたが知らせるまでは。",
  eligible:"応札できたメイカー",
  coverRound:"このラウンドはカバーでした。値段は使いません。",
  // maker
  makerTitle:"あなたの価格方針（あなただけが見ています）",
  makerWhy:"注文は見えません。方針だけを置き、見えない注文に対して評価されます。",
  ask_level:"売る値段", spread:"スプレッド", slope:"サイズ料", invcoef:"スキュー係数",
  inv:"在庫スキュー", maxqty:"受ける最大数量", active:"稼働", assetLabel:"建てる市場",
  invWhy:"＋で両サイドを上げる（買い戻したい）、−で下げる（売りたい）。",
  fillTitle:"約定", noFill:"今回、あなたへの通知はありません。",
  noFillWhy:"勝てなかったのか、そもそも別の銘柄だったのかは、あなたには分かりません。",
  yourFill:"あなたが取りました",
  tryTitle:"自分の板を試算する（手元だけの計算）",
  tryWhy:"この計算はブラウザの中だけで行われます。何も送っていません。",
  tryQty:"数量", ask:"売り気配", bid:"買い気配",
  // node
  nodeTitle:"あなたが持っているもの",
  nodeWhy:"注文とすべての方針の、あなたの持ち分。ここから元の値は出ません。",
  behaviourTitle:"あなたの振る舞い",
  behaviourWhy:"選んだことは、あなた以外の誰にも見えません。結果だけが公開されます。",
  verdictTitle:"今回の判定",
  namedYou:"あなたが名指しされました", namedNobody:"誰も名指しされていません",
  corrected:"訂正された開示", ofOpenings:"回 ／ 全",
  capacityProduct:"積の訂正能力", capacityOpen:"開示の訂正能力",
  rejectedYou:"あなたの持ち分はコミットメントと合いませんでした。計算の前に拒否されています。",
  // observer
  observerTitle:"神の視点",
  observerWhy:"実運用にこの画面はありません。部屋の前のスクリーン用です。",
  allQuotes:"全メイカーの気配", maker:"メイカー", node:"ノード",
  reason:"理由", winner:"勝者", price:"値段", request:"注文",
  behaviours:"ノードの振る舞い", settings:"設定",
  roundEvery:"自動ラウンド間隔（秒）", stepMs:"段階の間の待ち（ms）",
  autoRounds:"自動でラウンドを回す", inputCheck:"入力コミットメント検査",
  inputCheckWhy:"切ると、配られたのと違う入力を使うノードを誰も止められなくなります。",
  runNow:"いま1回まわす",
  // behaviours
  b_honest:"正直に計算する",
  b_honest_d:"何も起きません。これが基準です。",
  b_lie_product:"乗算の途中で違う持ち分を送る",
  b_lie_product_d:"整合しているが違うもの。訂正されて名指しされ、プロトコルは止まりません。能力を超えるまでは。",
  b_lie_open:"最後の開示で違う持ち分を送る",
  b_lie_open_d:"こちらも復号されて名指しされます。次数が低いぶん、耐えられる人数は多い。",
  b_lie_input:"配られたのと違う入力を使う",
  b_lie_input_d:"何も矛盾しません。コミットメント検査を切っていれば、誰も気づかず答えだけが狂います。",
  b_dropout:"入力のあと黙る",
  b_dropout_d:"評価点が1つ減るので、訂正できる嘘つきの数が減ります。",
  b_offline:"最初から参加しない",
  b_offline_d:"入力は全ノードの持ち分を足して作るので、1つ欠けると値そのものが消えます。ここは頑健ではありません。",
  inertTitle:"いまは MP-SPDZ が回しています",
  inertWhy:"この切り替えは届きません。",
  verifiedYes:"平文の照合と一致", verifiedNo:"平文の照合と不一致",
  protocolMs:"プロトコル時間", engRounds:"ラウンド数", engMb:"通信量 (MB)",
  compiledOnce:"最初の1回のコンパイル",
  n_finished:(f)=>`#${f.number} 完了（${f.real?"本物":"カバー"}）`,
  n_corrected:(f)=>`#${f.number} ${f.reductions} 回の開示のうち ${f.corrections} 回を訂正、`
    +`名指し：${f.named.map(n=>"ノード"+n).join("・")}。答えは出ました`,
  n_stopped:(f)=>`#${f.number} 停止 ---- ` + abortWhy(f),
  n_refused:(f)=>`${f.who.map(n=>"ノード"+n).join("・")} を計算の前に拒否しました`,
  n_unchecked:(f)=>`${f.who.map(n=>"ノード"+n).join("・")} が配られたのと違う入力を使いました。`
    +`何も矛盾しないので、誰も気づいていません。値段は間違っています`,
  n_claimed:(f)=>`${f.seat} に着席${f.label?"："+f.label:""}`,
  n_you_won:(f)=>`#${f.number} あなたが取りました`,
  a_beyond_capacity:(f)=>`応答 ${f.answered} 台・次数 ${f.degree} では ${f.capacity} 台までしか訂正できません`,
  a_absent:(f)=>`${f.who.map(n=>"ノード"+n).join("・")} が参加していません。入力は全 ${f.n} 台の持ち分の和なので、1 つ欠けると値そのものが消えます`,
  a_commitment:(f)=>`${f.who.map(n=>"ノード"+n).join("・")} の持ち分がコミットメントを開けません`,
  a_too_few:(f)=>`${f.answered} 台では次数 ${f.degree} の共有を復元できません（${f.needed} 台必要）`,
  explain:[
    ["テイカー","注文を出す人。注文は分割され、どのノードにも全体は渡りません。値段を受け取れるのはこの席だけです。"],
    ["メイカー","値段の方針を置く人。注文は見えません。勝ったことも、テイカーが知らせるまで分かりません。"],
    ["MPCノード","計算する人。持ち分しか持ちません。嘘をつくこともできます ---- 何が起きるかは、嘘の種類で3通りに分かれます。"]
  ]
},
en: {
  title:"QOMM", subtitle:"a best quote without showing the order",
  leave:"leave seat", langOther:"JA",
  lobbyTitle:"Take a seat",
  lobbyWhy:"Each seat sees something different. That difference is the argument. Any seat nobody is in runs itself.",
  yourName:"your name (optional)", watch:"watch (the view nobody has)",
  theSeats:"empty seats run themselves; named ones have a person in them",
  whatIsThis:"What this shows", publicTitle:"What was made public",
  publicWhy:"What anybody in the room can read after a round. No price in it.",
  seatsTitle:"Seats", seatsWhy:"Automatic or held. What a node intends to do is visible only to that node.",
  noticesTitle:"Notices for this seat",
  footer:"QOMM demo — ",
  taken:"taken", auto:"auto", manual:"held",
  phaseDeal:"deal", phaseCheck:"check", phaseReduce:"reduce", phaseOpen:"open",
  phaseIdle:"idle", phaseDone:"done",
  takerTitle:"Your order (only this seat sees it)",
  takerWhy:"Only shares leave this page. No node can put them back together.",
  asset:"market", side:"side", buy:"buy", sell:"sell", qty:"size",
  kind:"kind", real:"live", cover:"cover",
  coverWhy:"Cover runs the same circuit, the same bytes, the same time. Nothing outside this seat can tell the two apart.",
  submit:"send it", waiting:"running…",
  openedTitle:"What was opened",
  openedWhy:"Everyone in the room sees this number. You are the only one holding the mask.",
  minusMask:"subtract your mask",
  yourPrice:"your price", winnerIs:"the maker that won",
  noMaker:"no maker was eligible",
  announce:"tell the winner", announced:"told",
  announceWhy:"A maker does not know it won. Not until you say so.",
  eligible:"makers that could quote",
  coverRound:"that round was cover. The price is not used.",
  makerTitle:"Your price policy (only this seat sees it)",
  makerWhy:"You never see the order. You leave a policy and it is priced against something you are not shown.",
  ask_level:"ask level", spread:"spread", slope:"charge per unit", invcoef:"skew weight",
  inv:"inventory skew", maxqty:"largest size taken", active:"switched on",
  assetLabel:"market made",
  invWhy:"positive lifts both quotes (wants to buy back), negative drops them (wants to sell).",
  fillTitle:"Fill", noFill:"Nothing was said to you this round.",
  noFillWhy:"Whether you lost or the order was in another market, you cannot tell.",
  yourFill:"you took it",
  tryTitle:"Price your own book (local only)",
  tryWhy:"This runs in your browser. Nothing was sent.",
  tryQty:"size", ask:"ask", bid:"bid",
  nodeTitle:"What you hold",
  nodeWhy:"Your share of the order and of every policy. Nothing comes back out of it.",
  behaviourTitle:"What you will do",
  behaviourWhy:"Your choice is visible to nobody else. Only the result becomes public.",
  verdictTitle:"This round",
  namedYou:"you were named", namedNobody:"nobody was named",
  corrected:"openings corrected", ofOpenings:"of",
  capacityProduct:"products correct up to", capacityOpen:"the opening corrects up to",
  rejectedYou:"your share did not open its commitment. Refused before anything was computed.",
  observerTitle:"The view nobody has",
  observerWhy:"No deployment has this screen. It is for the one at the front of the room.",
  allQuotes:"every maker's quote", maker:"maker", node:"node",
  reason:"why", winner:"winner", price:"price", request:"order",
  behaviours:"node behaviour", settings:"settings",
  roundEvery:"automatic round every (s)", stepMs:"pause between steps (ms)",
  autoRounds:"run rounds automatically", inputCheck:"input commitment check",
  inputCheckWhy:"turn it off and nothing stops a node feeding a share other than the one it was dealt.",
  runNow:"run one now",
  b_honest:"compute honestly",
  b_honest_d:"nothing happens. This is the baseline.",
  b_lie_product:"send a wrong share during a multiplication",
  b_lie_product_d:"consistent-looking and wrong. Corrected, named, and the protocol does not stop --- until the capacity is passed.",
  b_lie_open:"send a wrong share at the final opening",
  b_lie_open_d:"decoded and named too. Lower degree, so more of you can do it.",
  b_lie_input:"feed a share other than the one you were dealt",
  b_lie_input_d:"nothing is inconsistent. With the commitment check off, nobody notices and the answer is simply wrong.",
  b_dropout:"go quiet after the inputs",
  b_dropout_d:"one fewer evaluation point, so fewer liars can be corrected.",
  b_offline:"never take part",
  b_offline_d:"inputs are the sum of every node's share, so one missing share destroys the value. This part is not robust.",
  inertTitle:"MP-SPDZ is running this",
  inertWhy:"this switch does not reach it.",
  verifiedYes:"matches the cleartext reference", verifiedNo:"does NOT match the cleartext reference",
  protocolMs:"protocol time", engRounds:"rounds", engMb:"traffic (MB)",
  compiledOnce:"compiled once",
  n_finished:(f)=>`#${f.number} done (${f.real?"live":"cover"})`,
  n_corrected:(f)=>`#${f.number} corrected ${f.corrections} of ${f.reductions} openings `
    +`and named ${f.named.map(n=>"node "+n).join(", ")}; the answer came out anyway`,
  n_stopped:(f)=>`#${f.number} stopped --- ` + abortWhy(f),
  n_refused:(f)=>`${f.who.map(n=>"node "+n).join(", ")} refused before anything was computed`,
  n_unchecked:(f)=>`${f.who.map(n=>"node "+n).join(", ")} fed a share other than the one it was `
    +`dealt. Nothing was inconsistent, so nobody noticed. The price is simply wrong`,
  n_claimed:(f)=>`${f.seat} taken${f.label?" by "+f.label:""}`,
  n_you_won:(f)=>`#${f.number} you took it`,
  a_beyond_capacity:(f)=>`${f.answered} answering at degree ${f.degree} corrects only ${f.capacity}`,
  a_absent:(f)=>`${f.who.map(n=>"node "+n).join(", ")} did not take part. Inputs are the sum of all ${f.n} shares, so one missing share destroys the value`,
  a_commitment:(f)=>`${f.who.map(n=>"node "+n).join(", ")} stated a share that does not open its commitment`,
  a_too_few:(f)=>`${f.answered} answering cannot rebuild a degree-${f.degree} sharing (${f.needed} needed)`,
  explain:[
    ["taker","sends the order. It is split, and no node is handed the whole of it. This seat alone gets a price."],
    ["maker","leaves a price policy. It never sees the order, and does not learn it won until the taker says so."],
    ["MPC node","computes. It holds shares and nothing else. It can also cheat --- and what happens then splits three ways."]
  ]
}};

/* An explicit choice wins, then whatever was chosen here before, then the
   browser's own setting. Defaulting to one language would leave half the
   audience reading a demo about disclosure in a language they did not pick. */
let lang = new URLSearchParams(location.search).get('lang')
  || localStorage.getItem('qomm.lang')
  || ((navigator.languages || [navigator.language || 'en'])
        .some(l => String(l).toLowerCase().startsWith('ja')) ? 'ja' : 'en');
function t(k){ return (S[lang] && S[lang][k]) !== undefined ? S[lang][k] : (S.ja[k] || k); }
/* A notice arrives as a code and its fields; the sentence is built here,
   because here is the only place that knows which language was chosen. */
function msg(code, fields){
  const f = t('n_' + code);
  if (typeof f === 'function'){ try { return f(fields || {}); } catch (e) {} }
  return code + ' ' + JSON.stringify(fields || {});
}
function abortWhy(fields){
  const f = t('a_' + (fields.why || fields.code || ''));
  if (typeof f === 'function'){ try { return f(fields); } catch (e) {} }
  return fields.detail || '';
}
function abortText(pub){
  if (!pub || !pub.aborted) return '';
  return abortWhy(Object.assign({why: pub.abort_code, detail: pub.abort_reason},
                                pub.abort_fields || {}));
}

const BEHAVIOURS = ['honest','lie_product','lie_open','lie_input','dropout','offline'];
const PHASES = [['deal','phaseDeal'],['check','phaseCheck'],['reduce','phaseReduce'],
                ['open','phaseOpen']];

let ws = null, V = null, built = '', sending = false, lastDraw = '';
let session = localStorage.getItem('qomm.session') || '';
const $ = (id) => document.getElementById(id);
const el = (tag, cls, txt) => { const e = document.createElement(tag);
  if (cls) e.className = cls; if (txt !== undefined) e.textContent = txt; return e; };

function send(msg){ if (ws && ws.readyState === 1) ws.send(JSON.stringify(msg)); }

function connect(){
  const q = new URLSearchParams();
  if (session) q.set('session', session);
  const url = new URLSearchParams(location.search);
  if (url.get('seat')) q.set('seat', url.get('seat'));
  if (url.get('label')) q.set('label', url.get('label'));
  ws = new WebSocket((location.protocol === 'https:' ? 'wss://' : 'ws://')
                     + location.host + '/ws?' + q.toString());
  ws.onmessage = (e) => { const m = JSON.parse(e.data);
    if (m.type === 'view'){ V = m; session = m.session;
      localStorage.setItem('qomm.session', session); render(); }
    else if (m.type === 'refused'){ alert(m.reason); } };
  ws.onclose = () => { $('conn').textContent = ' · reconnecting';
    setTimeout(connect, 1200); };
  ws.onopen = () => { $('conn').textContent = ''; };
}

/* ---------- number helpers ------------------------------------------- */
function showPrice(assets, index, ticks){
  if (ticks === null || ticks === undefined) return '--';
  const a = assets[index]; if (!a) return String(ticks);
  const digits = String(a.scale).length - 1;
  return (ticks / a.scale).toLocaleString(undefined,
    {minimumFractionDigits: digits, maximumFractionDigits: digits});
}
function setVal(node, value){
  if (node && document.activeElement !== node) node.value = value;
}

/* ---------- frame ----------------------------------------------------- */
function render(){
  if (!V) return;
  // The countdown moves every tick and nothing else does, so it is taken out
  // of the comparison and written on its own. Redrawing the whole page four
  // times a second would fight with anyone trying to scroll or drag a slider,
  // which in a room is the difference between a demo and a nuisance.
  $('countdown').textContent =
    (V.config.auto_rounds && V.next_round_in !== null && V.next_round_in !== undefined)
      ? '\u27f3 ' + V.next_round_in.toFixed(0) + 's' : '';
  const signature = JSON.stringify(V, (k, v) => k === 'next_round_in' ? 0 : v);
  if (signature === lastDraw) return;
  lastDraw = signature;
  document.documentElement.lang = lang;
  document.querySelectorAll('[data-s]').forEach(n => {
    const v = t(n.dataset.s); if (typeof v === 'string') n.textContent = v; });
  $('lang').textContent = t('langOther');
  const c = V.config;
  const eng = $('engine');
  eng.textContent = c.engine + ' · ' + c.engine_note;
  eng.className = 'badge engine' + (c.engine === 'sim' ? '' : ' mpc');
  $('roundno').textContent = (V.public && V.public.number)
    ? '#' + V.public.number + ' · ' + V.public.ms + ' ms' : '#--';
  const badge = $('seatbadge');
  badge.textContent = (V.seat || '') + (V.label ? ' · ' + V.label : '');
  badge.classList.toggle('hide', !V.seat);
  $('leave').classList.toggle('hide', !V.seat);
  $('lobby').classList.toggle('hide', !!V.seat);
  $('stage').classList.toggle('hide', !V.seat);
  if (!V.seat){ buildLobby(); return; }
  drawPhases();
  const want = V.kind + ':' + V.index;
  if (built !== want){ buildPanel(); built = want; }
  updatePanel();
  drawPublic();
  drawSeatMap();
  drawNotices();
}

function drawPhases(){
  const box = $('phases');
  if (box.children.length !== PHASES.length){
    box.textContent = '';
    PHASES.forEach(([id, key], i) => {
      const d = el('div', 'phase'); d.dataset.phase = id;
      d.appendChild(el('div', 'n', String(i + 1)));
      d.appendChild(el('div', 't', t(key)));
      box.appendChild(d);
    });
  }
  const order = PHASES.map(p => p[0]);
  const at = order.indexOf(V.phase);
  [...box.children].forEach((d, i) => {
    d.querySelector('.t').textContent = t(PHASES[i][1]);
    d.className = 'phase' + (i === at ? ' on' : (at < 0 || i < at ? ' done' : ''));
  });
  $('phasenote').textContent = V.phase_note || '';
}

function drawPublic(){
  const p = V.public || {}, box = $('publicbody');
  box.textContent = '';
  if (!p.number){ box.appendChild(el('div', 'why', '--')); return; }
  const key = el('div', 'leak-box');
  key.appendChild(el('div', 'tag leak', 'opened'));
  key.appendChild(el('div', 'opaque', p.masked_key));
  box.appendChild(key);
  const t2 = el('table');
  const rows = [
    [t('corrected'), p.corrections + ' / ' + p.reductions],
    [t('capacityProduct'), p.product_capacity],
    [t('capacityOpen'), p.open_capacity],
  ];
  if (p.named.length) rows.push([t('node'), p.named.map(n => '#' + n).join(', ')]);
  if (p.silent.length) rows.push(['silent', p.silent.map(n => '#' + n).join(', ')]);
  const st = p.engine_stats || {};
  if (st.protocol_ms !== undefined){
    rows.length = 0;                       // the engine reports its own numbers
    rows.push([t('protocolMs'), st.protocol_ms.toFixed(1) + ' ms'],
              [t('engRounds'), st.rounds], [t('engMb'), st.mb],
              [t('compiledOnce'), st.compiled_once_ms + ' ms']);
  }
  rows.forEach(([a, b]) => { const tr = el('tr');
    tr.appendChild(el('td', null, a)); tr.appendChild(el('td', 'mono', String(b)));
    t2.appendChild(tr); });
  box.appendChild(t2);
  if (p.verified !== null && p.verified !== undefined){
    const v = el('div', p.verified ? 'hidden-box' : 'bad-box');
    v.style.marginTop = '.5rem';
    v.appendChild(el('div', p.verified ? 'tag' : 'tag bad',
                     p.verified ? t('verifiedYes') : t('verifiedNo')));
    v.appendChild(el('div', 'why mono', p.verified_detail));
    box.appendChild(v);
  }
  if (p.aborted){ const bad = el('div', 'bad-box');
    bad.style.marginTop = '.5rem';
    bad.appendChild(el('div', 'tag bad', 'stopped'));
    bad.appendChild(el('div', null, abortText(p)));
    // The English detail stays underneath: it names the label and the degree,
    // and that is what somebody reading the transcript afterwards wants.
    bad.appendChild(el('div', 'why', p.abort_reason));
    box.appendChild(bad); }
}

function drawSeatMap(){
  const box = $('seatmap'); box.textContent = '';
  const named = new Set((V.public && V.public.named) || []);
  const silent = new Set((V.public && V.public.silent) || []);
  V.seats.forEach(s => {
    let cls = 'chip' + (s.mode === 'manual' ? ' manual' : '') + (s.mine ? ' mine' : '');
    if (s.kind === 'node' && named.has(s.index)) cls += ' named';
    if (s.kind === 'node' && silent.has(s.index)) cls += ' silent';
    const c = el('span', cls, s.id + (s.label ? ' · ' + s.label : ''));
    box.appendChild(c);
  });
}

function drawNotices(){
  const box = $('notices'); box.textContent = '';
  (V.notices || []).slice().reverse().forEach(n => {
    box.appendChild(el('div', n.tone, msg(n.code, n.fields))); });
}

/* ---------- lobby ----------------------------------------------------- */
function buildLobby(){
  const grid = $('lobbygrid'); grid.textContent = '';
  V.seats.forEach(s => {
    const b = el('button');
    b.appendChild(el('b', null, s.id));
    b.appendChild(el('span', null, s.held ? (s.label || t('taken')) : t('auto')));
    b.disabled = s.held;
    b.onclick = () => send({type:'claim', seat:s.id, value:'',
                            label:$('name').value});
    grid.appendChild(b);
  });
  const ex = $('explain'); ex.textContent = '';
  t('explain').forEach(([name, why]) => {
    const d = el('div'); d.style.marginBottom = '.4rem';
    const b = el('b', null, name + ' — '); d.appendChild(b);
    d.appendChild(document.createTextNode(why)); ex.appendChild(d); });
}

/* ---------- panels ---------------------------------------------------- */
function card(title, why){
  const c = el('div', 'card');
  if (title) c.appendChild(el('h2', null, title));
  if (why) c.appendChild(el('p', 'why', why));
  return c;
}
function slider(parent, label, id, min, max, onInput){
  const row = el('div', 'row');
  row.appendChild(el('label', null, label));
  const i = document.createElement('input');
  i.type = 'range'; i.min = min; i.max = max; i.id = id;
  const v = el('span', 'val', '');
  i.oninput = () => { v.textContent = i.value; onInput(Number(i.value)); };
  row.appendChild(i); row.appendChild(v); parent.appendChild(row);
  return i;
}
function segmented(parent, label, options, onPick, id){
  const row = el('div', 'row');
  if (label) row.appendChild(el('label', null, label));
  const seg = el('div', 'seg'); seg.id = id;
  options.forEach(o => { const b = el('button', o.tone || '', o.text);
    b.dataset.value = o.value; b.onclick = () => onPick(o.value); seg.appendChild(b); });
  row.appendChild(seg); parent.appendChild(row);
  return seg;
}
function pick(seg, value){
  if (!seg) return;
  [...seg.children].forEach(b => b.classList.toggle('on', b.dataset.value === String(value)));
}

function buildPanel(){
  const p = $('panel'); p.textContent = '';
  ({taker: buildTaker, maker: buildMaker, node: buildNode,
    observer: buildObserver}[V.kind] || (() => {}))(p);
}
function updatePanel(){
  ({taker: updateTaker, maker: updateMaker, node: updateNode,
    observer: updateObserver}[V.kind] || (() => {}))();
}

/* ---------- taker ------------------------------------------------------ */
let takerEls = {};
function buildTaker(root){
  const c = card(t('takerTitle'), t('takerWhy'));
  const assetRow = el('div', 'row');
  assetRow.appendChild(el('label', null, t('asset')));
  const sel = document.createElement('select');
  V.assets.forEach((a, i) => { const o = document.createElement('option');
    o.value = i; o.textContent = a.name + '  ' + showPrice(V.assets, i, a.reference);
    sel.appendChild(o); });
  sel.onchange = () => send({type:'request', values:{asset:Number(sel.value)}});
  assetRow.appendChild(sel); c.appendChild(assetRow);

  const side = segmented(c, t('side'), [
    {value:0, text:t('buy')}, {value:1, text:t('sell')}],
    (v) => send({type:'request', values:{direction:v}}), 'side');
  const qty = slider(c, t('qty'), 'qty', 1, 500,
    (v) => send({type:'request', values:{qty:v}}));
  const kind = segmented(c, t('kind'), [
    {value:1, text:t('real'), tone:'amber'}, {value:0, text:t('cover'), tone:'teal'}],
    (v) => send({type:'request', values:{is_real:v}}), 'kind');
  c.appendChild(el('div', 'note-box', t('coverWhy')));

  const go = el('button', 'go', t('submit'));
  go.style.marginTop = '.7rem';
  go.onclick = () => { sending = true; go.disabled = true; go.textContent = t('waiting');
    send({type:'submit'}); };
  c.appendChild(go);
  root.appendChild(c);

  const r = card(t('openedTitle'), t('openedWhy'));
  const opened = el('div', 'leak-box');
  opened.appendChild(el('div', 'tag leak', 'everyone sees this'));
  const maskedKey = el('div', 'opaque', '--');
  opened.appendChild(maskedKey); r.appendChild(opened);
  r.appendChild(el('div', 'why', '↓ ' + t('minusMask')));
  const priv = el('div', 'hidden-box');
  priv.appendChild(el('div', 'tag', t('yourPrice')));
  const price = el('div', 'big', '--'); priv.appendChild(price);
  const winner = el('div', null, ''); priv.appendChild(winner);
  r.appendChild(priv);
  const tell = el('button', 'go ghost', t('announce'));
  tell.style.marginTop = '.7rem';
  tell.onclick = () => send({type:'announce'});
  r.appendChild(tell);
  r.appendChild(el('p', 'why', t('announceWhy')));
  root.appendChild(r);
  takerEls = {sel, side, qty, kind, go, maskedKey, price, winner, tell};
}
function updateTaker(){
  const d = V.taker || {}, p = d.pending || {}, last = d.last, e = takerEls;
  setVal(e.sel, p.asset); pick(e.side, p.direction); pick(e.kind, p.is_real);
  setVal(e.qty, p.qty);
  if (e.qty && e.qty.parentElement)
    e.qty.parentElement.querySelector('.val').textContent = p.qty;
  const busy = V.phase !== 'done' && V.phase !== 'idle';
  e.go.disabled = busy;
  e.go.textContent = busy ? t('waiting') : t('submit');
  if (!last){ return; }
  e.maskedKey.textContent = (V.public && V.public.masked_key) || '--';
  if (V.public && V.public.aborted){
    e.price.textContent = '--'; e.winner.textContent = abortText(V.public);
    e.tell.classList.add('hide'); return;
  }
  if (last.winner === null || last.winner === undefined){
    e.price.textContent = '--'; e.winner.textContent = t('noMaker');
    e.tell.classList.add('hide'); return;
  }
  e.price.textContent = showPrice(V.assets, last.asset, last.price);
  e.winner.textContent = t('winnerIs') + ': maker:' + last.winner
    + '   ·   ' + t('eligible') + ' ' + last.eligible
    + (last.is_real ? '' : '   ·   ' + t('coverRound'));
  e.tell.classList.toggle('hide', !last.is_real);
  e.tell.disabled = last.announced;
  e.tell.textContent = last.announced ? t('announced') : t('announce');
}

/* ---------- maker ------------------------------------------------------ */
let makerEls = {};
const POLICY_RANGE = {ask_level:[-40,40], spread:[6,120], slope:[0,4], invcoef:[0,3],
                      inv:[-120,120], maxqty:[0,500]};
function buildMaker(root){
  const c = card(t('makerTitle'), t('makerWhy'));
  const assetRow = el('div', 'row');
  assetRow.appendChild(el('label', null, t('assetLabel')));
  const sel = document.createElement('select');
  V.assets.forEach((a, i) => { const o = document.createElement('option');
    o.value = i; o.textContent = a.name; sel.appendChild(o); });
  sel.onchange = () => send({type:'policy', values:{asset:Number(sel.value)}});
  assetRow.appendChild(sel); c.appendChild(assetRow);
  const sliders = {};
  Object.keys(POLICY_RANGE).forEach(name => {
    const [lo, hi] = POLICY_RANGE[name];
    sliders[name] = slider(c, t(name), 'p_' + name, lo, hi,
      (v) => { const values = {}; values[name] = v;
               send({type:'policy', values}); });
  });
  c.appendChild(el('div', 'note-box', t('invWhy')));
  const act = segmented(c, t('active'), [{value:1, text:'on'}, {value:0, text:'off'}],
    (v) => send({type:'policy', values:{active:v}}), 'active');
  root.appendChild(c);

  const f = card(t('fillTitle'), '');
  const fill = el('div'); f.appendChild(fill); root.appendChild(f);

  const local = card(t('tryTitle'), t('tryWhy'));
  const row = el('div', 'row');
  row.appendChild(el('label', null, t('tryQty')));
  const q = document.createElement('input');
  q.type = 'number'; q.value = 100; q.min = 1; q.max = 500;
  row.appendChild(q); local.appendChild(row);
  const out = el('table'); local.appendChild(out);
  q.oninput = () => drawLocal(out, Number(q.value));
  root.appendChild(local);
  makerEls = {sel, sliders, act, fill, out, q};
}
function drawLocal(out, qty){
  const pol = (V.maker && V.maker.policy) || null;
  out.textContent = '';
  if (!pol) return;
  const ref = V.assets[pol.asset] ? V.assets[pol.asset].reference : 0;
  const anchor = pol.use_ref * ref + pol.ask_level;
  const depth = pol.slope * qty, skew = pol.invcoef * pol.inv;
  [[t('ask'), anchor + depth + skew],
   [t('bid'), anchor - pol.spread - depth + skew]].forEach(([k, v]) => {
    const tr = el('tr'); tr.appendChild(el('td', null, k));
    tr.appendChild(el('td', 'mono', showPrice(V.assets, pol.asset, v)));
    out.appendChild(tr); });
  const warn = qty > pol.maxqty || !pol.active;
  const tr = el('tr'); tr.appendChild(el('td', null, ''));
  tr.appendChild(el('td', null, warn ? (pol.active ? '> ' + t('maxqty') : 'off') : ''));
  out.appendChild(tr);
}
function updateMaker(){
  const d = V.maker || {}, pol = d.policy, e = makerEls;
  if (!pol) return;
  setVal(e.sel, pol.asset);
  Object.keys(e.sliders).forEach(name => {
    setVal(e.sliders[name], pol[name]);
    e.sliders[name].parentElement.querySelector('.val').textContent = pol[name]; });
  pick(e.act, pol.active);
  e.fill.textContent = '';
  if (d.fill){
    const box = el('div', 'hidden-box');
    box.appendChild(el('div', 'tag', t('yourFill')));
    box.appendChild(el('div', 'big',
      showPrice(V.assets, d.fill.asset, d.fill.price)));
    box.appendChild(el('div', null,
      V.assets[d.fill.asset].name + '  ' + (d.fill.direction ? t('buy') : t('sell'))
      + '  ' + d.fill.qty));
    e.fill.appendChild(box);
  } else {
    e.fill.appendChild(el('div', 'why', t('noFill')));
    e.fill.appendChild(el('div', 'note-box', t('noFillWhy')));
  }
  drawLocal(e.out, Number(e.q.value));
}

/* ---------- node ------------------------------------------------------- */
let nodeEls = {};
function buildNode(root){
  const c = card(t('behaviourTitle'), t('behaviourWhy'));
  const list = el('div');
  const buttons = {};
  BEHAVIOURS.forEach(name => {
    const b = el('button');
    b.style.cssText = 'display:block;width:100%;text-align:left;margin-bottom:.3rem;'
      + 'border:1px solid var(--line);background:#fff;border-radius:8px;padding:.45rem .6rem';
    b.appendChild(el('b', null, t('b_' + name)));
    b.appendChild(el('div', 'why', t('b_' + name + '_d')));
    b.onclick = () => send({type:'behaviour', value:name});
    buttons[name] = b; list.appendChild(b);
  });
  c.appendChild(list); root.appendChild(c);

  const inert = el('div', 'note-box'); inert.classList.add('hide');
  inert.appendChild(el('b', null, t('inertTitle')));
  inert.appendChild(el('div', null, t('inertWhy')));
  c.insertBefore(inert, list);

  const s = card(t('nodeTitle'), t('nodeWhy'));
  const shares = el('div'); s.appendChild(shares); root.appendChild(s);

  const v = card(t('verdictTitle'), '');
  const verdict = el('div'); v.appendChild(verdict); root.appendChild(v);
  nodeEls = {buttons, shares, verdict, inert};
}
function updateNode(){
  const d = V.node || {}, p = V.public || {}, e = nodeEls;
  // Inert only when the engine cannot carry the switch. Under the robust
  // build a wrong share in a multiplication is a real party misbehaving.
  const inert = V.config.engine !== 'sim' && !V.config.robust;
  e.inert.classList.toggle('hide', !inert);
  BEHAVIOURS.forEach(name => {
    const on = d.behaviour === name;
    e.buttons[name].style.borderColor = on ? 'var(--blue)' : 'var(--line)';
    e.buttons[name].style.background = on ? 'var(--blue-pale)' : '#fff';
    e.buttons[name].style.boxShadow = on ? '0 0 0 2px var(--blue-pale)' : 'none';
    e.buttons[name].disabled = inert && name !== 'honest';
    e.buttons[name].style.opacity = (inert && name !== 'honest') ? '.45' : '1';
  });
  e.shares.textContent = '';
  (d.shares || []).forEach(h => e.shares.appendChild(el('div', 'opaque', h)));
  if (!(d.shares || []).length) e.shares.appendChild(el('div', 'why', '--'));

  e.verdict.textContent = '';
  if (d.rejected_me){
    const b = el('div', 'bad-box'); b.appendChild(el('div', 'tag bad', 'refused'));
    b.appendChild(el('div', null, t('rejectedYou'))); e.verdict.appendChild(b);
  } else if (d.named_me){
    const b = el('div', 'bad-box'); b.appendChild(el('div', 'tag bad', t('namedYou')));
    b.appendChild(el('div', 'big', d.times_named + ' / ' + p.reductions));
    e.verdict.appendChild(b);
  } else {
    const b = el('div', 'hidden-box');
    b.appendChild(el('div', 'tag', p.named && p.named.length
      ? 'node ' + p.named.join(', ') : t('namedNobody')));
    b.appendChild(el('div', 'big', (p.corrections || 0) + ' / ' + (p.reductions || 0)));
    b.appendChild(el('div', 'why', t('corrected')));
    e.verdict.appendChild(b);
  }
  if (p.aborted){ const bad = el('div', 'bad-box'); bad.style.marginTop = '.5rem';
    bad.appendChild(el('div', null, abortText(p)));
    bad.appendChild(el('div', 'why', p.abort_reason));
    e.verdict.appendChild(bad); }
}

/* ---------- observer --------------------------------------------------- */
let obsEls = {};
function buildObserver(root){
  const banner = el('div', 'note-box', t('observerWhy'));
  banner.style.marginBottom = '.8rem'; root.appendChild(banner);

  const c = card(t('allQuotes'), '');
  const head = el('div', 'row'); c.appendChild(head);
  const table = el('table'); c.appendChild(table); root.appendChild(c);

  const cfg = card(t('settings'), '');
  const rs = slider(cfg, t('roundEvery'), 'rs', 2, 60,
    (v) => send({type:'config', values:{round_seconds:v}}));
  const sm = slider(cfg, t('stepMs'), 'sm', 0, 1500,
    (v) => send({type:'config', values:{step_ms:v}}));
  const ar = segmented(cfg, t('autoRounds'), [{value:1, text:'on'}, {value:0, text:'off'}],
    (v) => send({type:'config', values:{auto_rounds:!!v}}), 'ar');
  const ic = segmented(cfg, t('inputCheck'), [{value:1, text:'on'}, {value:0, text:'off'}],
    (v) => send({type:'config', values:{input_check:!!v}}), 'ic');
  cfg.appendChild(el('div', 'note-box', t('inputCheckWhy')));
  const now = el('button', 'go', t('runNow'));
  now.style.marginTop = '.6rem';
  now.onclick = () => send({type:'submit_any'});
  cfg.appendChild(now);
  root.appendChild(cfg);

  const b = card(t('behaviours'), '');
  const behav = el('div', 'seatmap'); b.appendChild(behav); root.appendChild(b);
  obsEls = {head, table, rs, sm, ar, ic, behav};
}
function updateObserver(){
  const d = V.observer || {}, e = obsEls;
  e.head.textContent = '';
  if (d.request){
    const parts = [
      [t('request'), V.assets[d.request.asset].name + ' '
        + (d.request.direction ? t('sell') : t('buy')) + ' ' + d.request.qty
        + (d.request.is_real ? '' : ' · ' + t('cover'))],
      [t('winner'), d.winner === null ? '--' : 'maker:' + d.winner],
      [t('price'), showPrice(V.assets, d.request.asset, d.price)],
    ];
    parts.forEach(([k, v]) => { const box = el('div', 'hidden-box');
      box.style.flex = '1'; box.appendChild(el('div', 'tag', k));
      box.appendChild(el('div', null, v)); e.head.appendChild(box); });
  }
  e.table.textContent = '';
  const hr = el('tr');
  [t('maker'), t('assetLabel'), t('ask'), t('bid'), t('reason')].forEach(h =>
    hr.appendChild(el('th', null, h)));
  e.table.appendChild(hr);
  // Every quote is anchored on the reference of the market that was *asked
  // for*, whatever market its maker is in, so that is the scale they are all
  // shown at. Scaling each row by its own maker's market would print a BTC
  // price with two yen decimals and look like a bug in the circuit.
  const shownIn = d.request ? d.request.asset : 0;
  (d.quotes || []).forEach(q => {
    const tr = el('tr', q.maker === d.winner ? 'win' : (q.eligible ? '' : 'out'));
    tr.appendChild(el('td', null, 'maker:' + q.maker));
    tr.appendChild(el('td', null, (V.assets[q.asset] || {}).name || '--'));
    tr.appendChild(el('td', 'mono', showPrice(V.assets, shownIn, q.ask)));
    tr.appendChild(el('td', 'mono', showPrice(V.assets, shownIn, q.bid)));
    tr.appendChild(el('td', null, q.reason || ''));
    e.table.appendChild(tr);
  });
  const c = V.config;
  setVal(e.rs, c.round_seconds);
  e.rs.parentElement.querySelector('.val').textContent = c.round_seconds;
  setVal(e.sm, c.step_ms);
  e.sm.parentElement.querySelector('.val').textContent = c.step_ms;
  pick(e.ar, c.auto_rounds ? 1 : 0);
  pick(e.ic, c.input_check ? 1 : 0);
  e.behav.textContent = '';
  Object.keys(d.behaviours || {}).forEach(j => {
    const b = d.behaviours[j];
    e.behav.appendChild(el('span', 'chip' + (b === 'honest' ? '' : ' named'),
      'node:' + j + ' · ' + t('b_' + b)));
  });
}

/* ---------- boot ------------------------------------------------------- */
$('lang').onclick = () => { lang = lang === 'ja' ? 'en' : 'ja';
  localStorage.setItem('qomm.lang', lang); built = ''; lastDraw = ''; render(); };
$('leave').onclick = () => { send({type:'release'}); built = ''; };
$('watch').onclick = () => send({type:'claim', seat:'observer',
                                 label:$('name').value});
connect();
