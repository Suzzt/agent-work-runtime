/**
 * Team Web bridge helpers (WS-044).
 * Demo fixtures + optional proxy to awr-server `/v1/web` cookie entry.
 */
'use strict';

const fs = require('fs');
const path = require('path');

function asObject(body) {
  if (body == null) return {};
  if (typeof body === 'object') return body;
  try { return JSON.parse(body || '{}'); } catch { return null; }
}

function createTeamBridge(opts) {
  const TEAM = {
    url: opts.teamUrl || null,
    port: opts.port,
    fixtureDir:
      opts.teamFixtureDir ||
      path.resolve(__dirname, '../../tests/fixtures/workstreams/team-web-loop'),
    sessions: new Map(),
    receipts: new Map(),
  };

  function readFixture(name) {
    const file = path.join(TEAM.fixtureDir, name);
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  }

  function demoSessionCookie(id) {
    return `awr_web_session=${id}; HttpOnly; Path=/api/team; SameSite=Strict`;
  }

  function parseTeamCookie(req) {
    const raw = req.headers.cookie || '';
    for (const part of raw.split(';')) {
      const p = part.trim();
      if (p.startsWith('awr_web_session=')) {
        const id = p.slice('awr_web_session='.length).trim();
        if (/^[A-Za-z0-9_-]{1,200}$/.test(id)) return id;
      }
    }
    return null;
  }

  function teamAuth(req) {
    const id = parseTeamCookie(req);
    if (!id) return null;
    const session = TEAM.sessions.get(id);
    if (!session || session.revoked || session.expires_at_ms <= Date.now()) return null;
    return session;
  }

  async function proxyTeam(reqPath, req, body) {
    if (!TEAM.url) return null;
    const headers = {
      'content-type': 'application/json',
      'x-awr-web': '1',
      origin: `http://127.0.0.1:${TEAM.port}`,
    };
    if (req.headers.cookie) headers.cookie = req.headers.cookie;
    const payload =
      body == null || req.method === 'GET'
        ? undefined
        : typeof body === 'string'
          ? body
          : JSON.stringify(body);
    const res = await fetch(String(TEAM.url).replace(/\/$/, '') + reqPath, {
      method: req.method,
      headers,
      body: payload,
    });
    const text = await res.text();
    let json;
    try {
      json = JSON.parse(text);
    } catch {
      json = { ok: false, error: { code: 'BadGateway', message: text.slice(0, 200) } };
    }
    const setCookie = typeof res.headers.getSetCookie === 'function' ? res.headers.getSetCookie() : [];
    return { status: res.status, json, setCookie };
  }

  const routes = {
    'GET /api/team/projects': async (url, _body, req) => {
      if (TEAM.url) {
        const proxied = await proxyTeam('/v1/web/projects', req, null);
        if (proxied) return proxied.json;
      }
      const view = url.searchParams.get('view') === 'personal' ? 'personal-view.json' : 'team-view.json';
      const data = readFixture(view);
      const session =
        teamAuth(req) || { session_id: 'demo-anonymous', expires_at_ms: Date.now() + 3600000 };
      return { ok: true, projects: data.projects, session, view: data.view };
    },

    'GET /api/team/overview': async (url) => {
      const view = url.searchParams.get('view') === 'personal' ? 'personal-view.json' : 'team-view.json';
      const data = readFixture(view);
      return {
        ok: true,
        project: url.searchParams.get('project') || (data.projects[0] && data.projects[0].key),
        works: data.works,
        members: data.members,
        handoffs: data.handoffs || [],
        reviews: data.reviews || [],
        schema: data.schema,
      };
    },

    'POST /api/team/login': async (_url, body, req, res) => {
      if (TEAM.url) {
        const proxied = await proxyTeam('/v1/web/login', req, body);
        if (proxied) {
          for (const c of proxied.setCookie || []) res.setHeader('set-cookie', c);
          return proxied.json;
        }
      }
      const parsed = asObject(body);
      if (!parsed) {
        return { ok: false, error: { code: 'InvalidInput', message: 'invalid login' } };
      }
      if (!parsed.bearer || typeof parsed.bearer !== 'string' || parsed.bearer.length < 8) {
        return { ok: false, error: { code: 'Forbidden', message: 'access denied' } };
      }
      const id = 'ws_demo_' + Date.now().toString(36);
      const session = {
        session_id: id,
        bearer_present: true,
        expires_at_ms: Date.now() + 8 * 3600 * 1000,
        revoked: false,
      };
      TEAM.sessions.set(id, session);
      res.setHeader('set-cookie', demoSessionCookie(id));
      return {
        ok: true,
        protocol: 'awr-team-web-entry',
        protocol_version: 1,
        session_id: id,
        expires_at_ms: session.expires_at_ms,
        auth: { kind: 'http_only_cookie', bearer_in_page: false },
      };
    },

    'POST /api/team/logout': async (_url, _body, req, res) => {
      if (TEAM.url) {
        const proxied = await proxyTeam('/v1/web/logout', req, '{}');
        if (proxied) {
          for (const c of proxied.setCookie || []) res.setHeader('set-cookie', c);
          return proxied.json;
        }
      }
      const id = parseTeamCookie(req);
      if (id && TEAM.sessions.has(id)) TEAM.sessions.get(id).revoked = true;
      res.setHeader(
        'set-cookie',
        'awr_web_session=; HttpOnly; Path=/api/team; SameSite=Strict; Max-Age=0'
      );
      return { ok: true, logged_out: true };
    },

    'POST /api/team/session/revoke': async (_url, body, req, res) => {
      if (TEAM.url) {
        const proxied = await proxyTeam('/v1/web/session/revoke', req, body);
        if (proxied) {
          for (const c of proxied.setCookie || []) res.setHeader('set-cookie', c);
          return proxied.json;
        }
      }
      const id = parseTeamCookie(req);
      if (id && TEAM.sessions.has(id)) TEAM.sessions.get(id).revoked = true;
      for (const s of TEAM.sessions.values()) s.revoked = true;
      res.setHeader(
        'set-cookie',
        'awr_web_session=; HttpOnly; Path=/api/team; SameSite=Strict; Max-Age=0'
      );
      return { ok: true, revoked: [{ scope: 'all_mine' }] };
    },

    'POST /api/team/action': async (_url, body) => {
      const parsed = asObject(body);
      if (!parsed) {
        return { ok: false, error: { code: 'InvalidInput', message: 'invalid action' } };
      }
      const allowed = new Set([
        'accept_responsibility',
        'select_agent',
        'respond_blocker',
        'handoff_receive',
        'submit_review',
        'rework',
        'accept',
      ]);
      if (!allowed.has(parsed.action) || !parsed.request_id || !parsed.work_key) {
        return { ok: false, error: { code: 'InvalidInput', message: 'invalid action fields' } };
      }
      if (parsed.expired === true) {
        return { ok: false, error: { code: 'ExpiredOperation', message: 'operation expired' } };
      }
      const receiptKey = parsed.request_id;
      if (TEAM.receipts.has(receiptKey)) {
        return { ok: true, replayed: true, receipt: TEAM.receipts.get(receiptKey) };
      }
      const opMap = {
        accept_responsibility: 'handoff.accept',
        select_agent: 'claim.acquire',
        respond_blocker: 'session.checkpoint',
        handoff_receive: 'handoff.accept',
        submit_review: 'delivery.submit_and_request_review',
        rework: 'work.rework',
        accept: 'review.accept',
      };
      const receipt = {
        id: 'rcpt_' + receiptKey,
        request_id: receiptKey,
        op: opMap[parsed.action],
        work_key: parsed.work_key,
        project: parsed.project,
        agent_id: parsed.agent_id || null,
        at_ms: Date.now(),
      };
      TEAM.receipts.set(receiptKey, receipt);
      return { ok: true, replayed: false, receipt, server_op: receipt.op };
    },
  };

  return { TEAM, routes, readFixture, teamAuth };
}

module.exports = { createTeamBridge };
