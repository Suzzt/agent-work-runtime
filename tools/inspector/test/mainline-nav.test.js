/**
 * Load mainline must be wired when the console starts, not inside Refresh.
 *
 * Run: node --test test/mainline-nav.test.js
 */

'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');

require('./fixtures/dom-stub.js').install();
require('../public/demo-data.js');

let start;
document.addEventListener = (type, fn) => {
  if (type === 'DOMContentLoaded') start = fn;
};

require('../public/app.js');

const json = (data) => ({ json: async () => data });

test('Load mainline is bound at startup and Refresh does not stack another listener', async () => {
  const urls = [];
  global.fetch = async (url) => {
    urls.push(String(url));
    if (String(url).includes('/api/mainline-nav')) {
      return json({
        ok: true,
        command: 'mainlineNav',
        data: {
          accounting: {
            available: true,
            required_count: 4,
            planned: { recorded: 1, not_met: 0, unknown: 0 },
          },
          blockers: [{ work: 'W1', blocker: 'needs review' }],
          cross_dependencies: [],
          mainline_graph: { nodes: [], edges: [] },
          guidance: { when: 'now', basis: 'scope', next_action: 'review W1', recheck: 'after review' },
        },
      });
    }
    if (url === '/api/health') return json({ ok: true, data: { mode: 'demo' } });
    return json({ ok: true, data: {} });
  };

  assert.equal(typeof start, 'function');
  await start();

  const load = document.getElementById('navLoadBtn');
  assert.ok(load, 'navLoadBtn must exist');
  assert.equal(
    (load.listeners.click || []).length,
    1,
    'Load mainline must have one click listener after startup, before Refresh',
  );

  await load.click();
  assert.ok(
    urls.some((url) => url.includes('/api/mainline-nav')),
    'clicking Load mainline before Refresh must call the mainline API',
  );
  const acc = document.getElementById('navAccBody').textContent;
  assert.match(acc, /Required work count: 4/);
  assert.match(acc, /W1: needs review/);
  assert.doesNotMatch(acc, /Accounting unavailable/);
  assert.match(document.getElementById('navGuideBody').textContent, /when: now/i);

  const refresh = document.getElementById('btnRefresh');
  await refresh.click();
  await refresh.click();
  assert.equal(
    (load.listeners.click || []).length,
    1,
    'Refresh must not register another Load mainline listener',
  );
});
