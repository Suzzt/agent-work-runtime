/**
 * A minimal DOM stub.
 *
 * Runs the actual app.js detail-rendering path in Node, without implementing
 * the full DOM specification; only the methods used by that path are included.
 *
 * Usage: call install() before require('../public/app.js').
 */

'use strict';

class StubElement {
  constructor(tag) {
    this.tagName = String(tag || 'div').toUpperCase();
    this.className = '';
    this.children = [];
    this.style = {};
    this.dataset = {};
    this.attributes = {};
    this.listeners = {};
    this.hidden = false;
    this.value = '';
    this.own = ''; // Own text, excluding child nodes.
  }

  set textContent(v) {
    this.own = v == null ? '' : String(v);
    this.children = [];
  }

  get textContent() {
    return this.own + this.children.map((c) => c.textContent).join('');
  }

  get innerText() {
    return this.textContent;
  }

  get firstChild() {
    return this.children.length ? this.children[0] : null;
  }

  appendChild(child) {
    this.children.push(child);
    return child;
  }

  removeChild(child) {
    this.children = this.children.filter((c) => c !== child);
    return child;
  }

  setAttribute(name, value) {
    this.attributes[name] = String(value);
  }

  getAttribute(name) {
    return Object.prototype.hasOwnProperty.call(this.attributes, name) ? this.attributes[name] : null;
  }

  addEventListener(type, fn) {
    (this.listeners[type] = this.listeners[type] || []).push(fn);
  }

  click() {
    for (const fn of this.listeners.click || []) fn({ target: this });
  }

  querySelectorAll() {
    return [];
  }

  querySelector() {
    return null;
  }

  /** Recursively find the first matching descendant, such as a test button. */
  find(predicate) {
    for (const child of this.children) {
      if (predicate(child)) return child;
      const hit = child.find(predicate);
      if (hit) return hit;
    }
    return null;
  }
}

/** Element IDs requested by app.js through getElementById. */
const IDS = [
  'workDetail', 'detailId', 'detailStatus', 'rawWorkBody', 'rawWorkBody',
  'fWork', 'fGoal', 'fBudget', 'fIntent', 'cliMirror',
  'view-overview', 'view-work', 'view-context', 'view-sources',
  'workRows', 'workFilters', 'workSub', 'workEmpty',
];

function install() {
  const byId = new Map();
  for (const id of IDS) byId.set(id, new StubElement('div'));

  const document = {
    getElementById: (id) => {
      if (!byId.has(id)) byId.set(id, new StubElement('div'));
      return byId.get(id);
    },
    createElement: (tag) => new StubElement(tag),
    createTextNode: (text) => {
      const node = new StubElement('#text');
      node.textContent = text;
      return node;
    },
    querySelectorAll: () => [],
    querySelector: () => null,
    addEventListener: () => {},
    documentElement: new StubElement('html'),
  };

  global.document = document;
  global.window = {
    AWR_DEMO: undefined,
    scrollTo: () => {},
    matchMedia: () => ({ matches: false }),
    confirm: () => true,
    addEventListener: () => {},
  };
  global.history = { replaceState: () => {} };
  global.location = { hash: '' };
  global.localStorage = {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  };
  global.sessionStorage = global.localStorage;
  // Node exposes a read-only navigator; replace it with defineProperty.
  Object.defineProperty(global, 'navigator', {
    value: { clipboard: null },
    configurable: true,
    writable: true,
  });

  return { document, byId };
}

module.exports = { install, StubElement };
