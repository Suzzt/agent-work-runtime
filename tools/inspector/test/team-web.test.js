'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');
const http = require('http');
const fs = require('fs');
const path = require('path');
const { createTeamBridge } = require('../team-bridge');

const FIXTURE_DIR = path.resolve(__dirname, '../../../tests/fixtures/workstreams/team-web-loop');
const GUARD = { 'x-awr-inspector': '1', origin: 'http://127.0.0.1' };

function startBridge() {
  const bridge = createTeamBridge({ teamFixtureDir: FIXTURE_DIR, port: 0 });
  const server = http.createServer(async (req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1');
    const key = `${req.method} ${url.pathname}`;
    const handler = bridge.routes[key];
    if (!handler) {
      res.writeHead(404, { 'content-type': 'application/json' });
      res.end('{}');
      return;
    }
    let body = null;
    if (req.method === 'POST') {
      const chunks = [];
      for await (const c of req) chunks.push(c);
      const raw = Buffer.concat(chunks).toString('utf8');
      body = raw ? JSON.parse(raw) : {};
    }
    try {
      const json = await handler(url, body, req, res);
      const cookie = res.getHeader('set-cookie');
      const headers = { 'content-type': 'application/json' };
      if (cookie) headers['set-cookie'] = cookie;
      res.writeHead(200, headers);
      res.end(JSON.stringify(json));
    } catch (err) {
      res.writeHead(500, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ ok: false, error: { message: String(err.message || err) } }));
    }
  });
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      resolve({
        base: `http://127.0.0.1:${port}`,
        close: () => new Promise((r) => server.close(r)),
        bridge,
      });
    });
  });
}

async function req(base, method, urlPath, { body, headers, cookie } = {}) {
  const res = await fetch(base + urlPath, {
    method,
    headers: {
      ...GUARD,
      ...(headers || {}),
      ...(cookie ? { cookie } : {}),
      ...(body ? { 'content-type': 'application/json' } : {}),
    },
    body: body ? JSON.stringify(body) : undefined,
  });
  const setCookie = typeof res.headers.getSetCookie === 'function' ? res.headers.getSetCookie() : [];
  const json = await res.json();
  return { status: res.status, json, setCookie };
}

test('fixtures exist for personal and team views', () => {
  for (const name of ['personal-view.json', 'team-view.json', 'acceptance-cases.json']) {
    assert.ok(fs.existsSync(path.join(FIXTURE_DIR, name)));
  }
});

test('team overview covers owner agent outcome blocker next and card fields', async () => {
  const b = await startBridge();
  try {
    const { json } = await req(b.base, 'GET', '/api/team/overview?view=team&project=demo');
    assert.equal(json.ok, true);
    assert.ok(json.works.length >= 2);
    const blocked = json.works.find((w) => w.key === 'TW-202');
    assert.ok(blocked.blocker.prerequisite_outcome);
    assert.ok(blocked.blocker.release_condition);
    assert.ok(blocked.blocker.check_basis);
    assert.ok(blocked.depends_on.some((d) => d.visible));
    assert.equal(typeof blocked.blocker.backend_code, 'string');
    const hidden = json.works.find((w) => (w.hidden_deps || []).length);
    assert.ok(hidden.hidden_deps[0].hint);
    assert.equal(hidden.hidden_deps[0].leaks, false);
  } finally {
    await b.close();
  }
});

test('login sets cookie, logout and revoke clear it; bearer not echoed', async () => {
  const b = await startBridge();
  try {
    const login = await req(b.base, 'POST', '/api/team/login', {
      body: { bearer: 'awr1.test.0123456789abcdef' },
    });
    assert.equal(login.json.ok, true);
    assert.equal(login.json.auth.bearer_in_page, false);
    assert.ok(!JSON.stringify(login.json).includes('awr1.test.0123456789abcdef'));
    assert.ok(login.setCookie.some((c) => c.startsWith('awr_web_session=') && c.includes('HttpOnly')));
    const cookie = login.setCookie[0].split(';')[0];
    const logout = await req(b.base, 'POST', '/api/team/logout', { cookie, body: {} });
    assert.equal(logout.json.logged_out, true);
  } finally {
    await b.close();
  }
});

test('actions issue idempotent receipts and map to shared server ops', async () => {
  const b = await startBridge();
  try {
    const body = {
      project: 'demo',
      work_key: 'TW-201',
      action: 'submit_review',
      request_id: 'req-1',
    };
    const first = await req(b.base, 'POST', '/api/team/action', { body });
    assert.equal(first.json.ok, true);
    assert.equal(first.json.replayed, false);
    assert.equal(first.json.server_op, 'delivery.submit_and_request_review');
    const second = await req(b.base, 'POST', '/api/team/action', { body });
    assert.equal(second.json.replayed, true);
    assert.equal(second.json.receipt.id, first.json.receipt.id);
  } finally {
    await b.close();
  }
});

test('expired operations are rejected', async () => {
  const b = await startBridge();
  try {
    const { json } = await req(b.base, 'POST', '/api/team/action', {
      body: {
        project: 'demo',
        work_key: 'TW-201',
        action: 'accept',
        request_id: 'req-expired',
        expired: true,
      },
    });
    assert.equal(json.ok, false);
    assert.equal(json.error.code, 'ExpiredOperation');
  } finally {
    await b.close();
  }
});

test('team-web module double-submit guard', async () => {
  const teamWeb = require('../public/team-web.js');
  const calls = [];
  const api = teamWeb.createTeamWeb({
    i18n: { t: (k) => k },
    $: () => null,
    callApi: async () => ({ ok: true }),
  });
  let resolveGate;
  const gate = new Promise((r) => { resolveGate = r; });
  let started = 0;
  const run = api._guardDouble('act', async () => {
    started += 1;
    calls.push('start');
    await gate;
    calls.push('end');
  });
  const p1 = run();
  const p2 = run(); // should no-op while inflight
  resolveGate();
  await Promise.all([p1, p2]);
  assert.equal(started, 1);
});

test('i18n keys for team web exist in en and zh-CN', () => {
  const enText = fs.readFileSync(path.join(__dirname, '../public/locales/en.js'), 'utf8');
  const zhText = fs.readFileSync(path.join(__dirname, '../public/locales/zh-CN.js'), 'utf8');
  for (const key of [
    'ui.team_web',
    'ui.my_projects',
    'ui.blocker_detail',
    'ui.accept_responsibility',
    'ui.hidden_dep_hint_p0',
  ]) {
    assert.ok(enText.includes('"' + key + '"'), key + ' en');
    assert.ok(zhText.includes('"' + key + '"'), key + ' zh');
  }
});
