// A DOM small enough to read, large enough to run demo.js: elements, class
// lists, data-* attributes, a few selectors, and a fake WebSocket.  It parses
// the real index.html so that the ids and data-s hooks under test are the
// page's own, not a copy.
export class ClassList {
  constructor(el){ this.el = el; }
  _list(){ return (this.el.attributes.class || '').split(/\s+/).filter(Boolean); }
  _set(list){ this.el.attributes.class = list.join(' '); }
  contains(c){ return this._list().includes(c); }
  add(...cs){ const l = this._list(); cs.forEach(c => { if (!l.includes(c)) l.push(c); }); this._set(l); }
  remove(...cs){ this._set(this._list().filter(c => !cs.includes(c))); }
  toggle(c, force){
    const on = force === undefined ? !this.contains(c) : !!force;
    if (on) this.add(c); else this.remove(c);
    return on;
  }
}

const camel = (s) => s.replace(/-([a-z])/g, (_, c) => c.toUpperCase());

export class TextNode {
  constructor(doc, text){ this.ownerDocument = doc; this.nodeType = 3; this.data = String(text); this.parentElement = null; }
  get textContent(){ return this.data; }
  set textContent(v){ this.data = String(v); }
}

export class Element {
  constructor(doc, tag, ns){
    this.ownerDocument = doc; this.nodeType = 1; this.tagName = tag; this.namespaceURI = ns || null;
    this.childNodes = []; this.attributes = {}; this.style = {}; this.dataset = {}; this._listeners = {};
    this.classList = new ClassList(this); this.parentElement = null;
    this.disabled = false; this.value = ''; this.scrollTop = 0;
  }
  get children(){ return this.childNodes.filter(n => n.nodeType === 1); }
  get id(){ return this.attributes.id || ''; }
  set id(v){ this.setAttribute('id', v); }
  get className(){ return this.attributes.class || ''; }
  set className(v){ this.attributes.class = String(v); }
  get textContent(){ return this.childNodes.map(n => n.textContent).join(''); }
  set textContent(v){
    this.childNodes.forEach(n => { n.parentElement = null; });
    this.childNodes = [];
    if (v !== '' && v !== null && v !== undefined) this.appendChild(this.ownerDocument.createTextNode(v));
  }
  get innerHTML(){ return this.textContent; }
  set innerHTML(v){ this.textContent = v; }
  appendChild(n){ if (n.parentElement) n.parentElement.removeChild(n); n.parentElement = this; this.childNodes.push(n); return n; }
  removeChild(n){ const i = this.childNodes.indexOf(n); if (i >= 0) this.childNodes.splice(i, 1); n.parentElement = null; return n; }
  insertBefore(n, ref){
    if (n.parentElement) n.parentElement.removeChild(n);
    const i = ref ? this.childNodes.indexOf(ref) : -1;
    n.parentElement = this;
    if (i < 0) this.childNodes.push(n); else this.childNodes.splice(i, 0, n);
    return n;
  }
  setAttribute(k, v){ this.attributes[k] = String(v); if (k.startsWith('data-')) this.dataset[camel(k.slice(5))] = String(v); }
  setAttributeNS(_ns, k, v){ this.setAttribute(k.replace(/^.*:/, ''), v); }
  getAttribute(k){ return k in this.attributes ? this.attributes[k] : null; }
  hasAttribute(k){ return k in this.attributes; }
  removeAttribute(k){ delete this.attributes[k]; }
  addEventListener(type, fn){ (this._listeners[type] = this._listeners[type] || []).push(fn); }
  removeEventListener(type, fn){ this._listeners[type] = (this._listeners[type] || []).filter(f => f !== fn); }
  dispatch(type, ev){
    const event = Object.assign({type, target: this, preventDefault(){}, stopPropagation(){}}, ev || {});
    (this._listeners[type] || []).forEach(fn => fn.call(this, event));
    const handler = this['on' + type];
    if (typeof handler === 'function') handler.call(this, event);
  }
  click(){ if (!this.disabled) this.dispatch('click'); }
  focus(){ this.ownerDocument.activeElement = this; }
  blur(){ this.ownerDocument.activeElement = null; }
  get clientWidth(){ return this.id === 'network-graph' ? this.ownerDocument.defaultView.__graphWidth : 800; }
  get clientHeight(){ return 400; }
  get scrollHeight(){ return 100; }
  querySelector(sel){ return this.querySelectorAll(sel)[0] || null; }
  querySelectorAll(sel){
    const parts = sel.trim().split(/\s+/);
    const out = [];
    walk(this, (e) => {
      if (!simpleMatch(e, parts[parts.length - 1])) return;
      let node = e, ok = true;
      for (let i = parts.length - 2; i >= 0; i--){
        node = node.parentElement;
        while (node && node !== this && !simpleMatch(node, parts[i])) node = node.parentElement;
        if (!node || node === this){ ok = false; break; }
      }
      if (ok) out.push(e);
    });
    return out;
  }
  closest(sel){ let n = this; while (n){ if (simpleMatch(n, sel)) return n; n = n.parentElement; } return null; }
  matches(sel){ return simpleMatch(this, sel); }
}

function simpleMatch(e, sel){
  if (e.nodeType !== 1) return false;
  const m = /^([a-z0-9-]*)((?:[.#][^.#\[]+|\[[^\]]+\])*)$/i.exec(sel);
  if (!m) return false;
  if (m[1] && e.tagName !== m[1].toLowerCase()) return false;
  const rest = m[2] || '';
  const re = /([.#])([^.#\[]+)|\[([^\]=]+)(?:="?([^"\]]*)"?)?\]/g;
  let p;
  while ((p = re.exec(rest))){
    if (p[1] === '.' && !e.classList.contains(p[2])) return false;
    if (p[1] === '#' && e.id !== p[2]) return false;
    if (p[3] && !(e.hasAttribute(p[3]) && (p[4] === undefined || e.getAttribute(p[3]) === p[4]))) return false;
  }
  return true;
}
function walk(root, fn){ root.childNodes.forEach(n => { if (n.nodeType === 1){ fn(n); walk(n, fn); } }); }

export function parseHtml(html, doc){
  const root = doc.createElement('html');
  const stack = [root];
  const VOID = new Set(['meta', 'link', 'input', 'br', 'img', 'hr']);
  const re = /<!--[\s\S]*?-->|<!DOCTYPE[^>]*>|<\/([a-zA-Z0-9-]+)\s*>|<([a-zA-Z0-9-]+)((?:\s+[a-zA-Z:-]+(?:="[^"]*")?)*)\s*(\/?)>|([^<]+)/g;
  let m;
  while ((m = re.exec(html))){
    if (m[1]){
      const tag = m[1].toLowerCase();
      for (let i = stack.length - 1; i > 0; i--){ if (stack[i].tagName === tag){ stack.length = i; break; } }
    } else if (m[2]){
      const tag = m[2].toLowerCase();
      const e = doc.createElement(tag);
      const ar = /([a-zA-Z:-]+)(?:="([^"]*)")?/g; let a;
      while ((a = ar.exec(m[3] || ''))) e.setAttribute(a[1], a[2] === undefined ? '' : a[2]);
      stack[stack.length - 1].appendChild(e);
      if (!VOID.has(tag) && !m[4]) stack.push(e);
    } else if (m[5] && m[5].trim()){
      stack[stack.length - 1].appendChild(doc.createTextNode(m[5]));
    }
  }
  return root;
}

export class FakeWebSocket {
  constructor(url){ this.url = url; this.readyState = 1; this.sent = []; FakeWebSocket.instances.push(this); }
  send(text){ this.sent.push(JSON.parse(text)); }
  close(){ this.readyState = 3; if (this.onclose) this.onclose({}); }
  deliver(message){ this.onmessage({data: JSON.stringify(message)}); }
}
FakeWebSocket.instances = [];

export function makeWindow(html, options){
  const opts = Object.assign({width: 1000, reducedMotion: false, search: '', lang: 'ja'}, options || {});
  const doc = {
    activeElement: null,
    createElement(tag){ return new Element(doc, tag.toLowerCase()); },
    createElementNS(ns, tag){ return new Element(doc, tag, ns); },
    createTextNode(text){ return new TextNode(doc, text); },
    getElementById(id){ let found = null; walk(doc.documentElement, e => { if (!found && e.id === id) found = e; }); return found; },
    querySelector(sel){ return doc.documentElement.querySelector(sel); },
    querySelectorAll(sel){ return doc.documentElement.querySelectorAll(sel); },
  };
  doc.documentElement = parseHtml(html, doc);
  doc.body = doc.documentElement.querySelector('body');
  const store = new Map();
  const window = {
    __graphWidth: opts.width,
    document: doc,
    localStorage: {getItem: (k) => (store.has(k) ? store.get(k) : null), setItem: (k, v) => store.set(k, String(v)), removeItem: (k) => store.delete(k)},
    location: {search: opts.search, protocol: 'http:', host: '127.0.0.1:8801'},
    navigator: {languages: [opts.lang], language: opts.lang},
    matchMedia: () => ({matches: opts.reducedMotion, addEventListener(){}, addListener(){}}),
    addEventListener(type, fn){ (window._listeners = window._listeners || {})[type] = fn; },
    WebSocket: FakeWebSocket,
    setTimeout, clearTimeout, console, URLSearchParams,
  };
  // The legacy controller is unit-tested in this deliberately small DOM.
  // React Flow itself is type-checked, production-built and exercised in the
  // real-browser acceptance test; this bridge records the exact model passed
  // across that boundary and exposes a semantic DOM for controller assertions.
  window.QommNetworkGraph = {
    render(container, model, graphOptions){
      container.textContent = '';
      container._qommGraph = {model, options: graphOptions};
      const nodes = doc.createElement('div'); nodes.id = 'network-nodes';
      model.nodes.forEach(item => {
        const node = doc.createElement('article');
        node.className = ['gnode', item.type, ...(item.classes || [])].join(' ');
        node.style.left = String(item.x - item.w / 2);
        node.style.width = String(item.w);
        node.setAttribute('aria-label', [item.title, item.sub, item.badge].filter(Boolean).join(' — '));
        node.appendChild(doc.createTextNode([item.title, item.sub, item.badge].filter(Boolean).join(' ')));
        for (const metric of item.metrics || []) node.appendChild(doc.createTextNode(` ${metric.label} ${metric.value}`));
        nodes.appendChild(node);
      });
      const edges = doc.createElement('div'); edges.id = 'graph-edges';
      const particles = doc.createElement('div'); particles.id = 'graph-particles';
      model.edges.forEach(item => {
        const edge = doc.createElement('span');
        edge.className = ['gedge', item.state === 'flow' ? 'is-flow' : item.state === 'done' ? 'is-done' : item.state === 'cut' ? 'is-off' : '', item.color, item.own ? '' : 'faint'].filter(Boolean).join(' ');
        edges.appendChild(edge);
        if (item.state === 'flow' && item.own && item.particle && !graphOptions.reducedMotion)
          particles.appendChild(doc.createElement('i'));
      });
      const empty = doc.createElement('p'); empty.id = 'graph-empty';
      empty.className = graphOptions.noRoundText ? 'graph-empty' : 'graph-empty hide';
      empty.textContent = graphOptions.noRoundText;
      const notes = doc.createElement('aside'); notes.className = 'qrf-notes';
      notes.textContent = (graphOptions.legendNotes || []).join(' ');
      container.appendChild(edges);
      container.appendChild(particles);
      container.appendChild(nodes);
      container.appendChild(empty);
      container.appendChild(notes);
    },
  };
  doc.defaultView = window;
  window.window = window;
  return window;
}
