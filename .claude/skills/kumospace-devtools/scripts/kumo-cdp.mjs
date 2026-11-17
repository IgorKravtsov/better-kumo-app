#!/usr/bin/env node
import { appendFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const BASE = (process.env.KUMO_CDP || 'http://127.0.0.1:9222').replace(/\/$/, '');
const SECRET_KEY = /token|secret|password|authorization|cookie|session|apikey|api_key|signature/i;
const OFFICE_SOCKET = 'wss://ws.kumospace.com';

const FIND_STORE = `(() => {
  const root = document.getElementById("root") || document.body.firstElementChild;
  const key = root && Object.keys(root).find((k) => k.startsWith("__reactContainer$"));
  if (!key) return null;
  const stack = [root[key]];
  while (stack.length) {
    const n = stack.pop();
    if (!n) continue;
    const p = n.memoizedProps;
    if (p && p.store && typeof p.store.getState === "function") return p.store;
    if (n.child) stack.push(n.child);
    if (n.sibling) stack.push(n.sibling);
  }
  return null;
})()`;

function usage() {
  console.log(`kumo-cdp <command> [options]   (KUMO_CDP=${BASE})

  targets                              list DevTools targets
  tail [--seconds N] [--out FILE] [--http] [--bodies] [--all-sockets] [--raw]
                                       log Office WebSocket frames (and HTTP with --http) as JSON lines
  state [PATH]                         print the Redux state, or one dotted path (e.g. users.userSelf)
  eval EXPRESSION                      evaluate a read-only expression in the page and print the result
  grep TERM... [--context N] [--max N] search the loaded Kumospace JS bundles
  dump DIR                             save every loaded Kumospace JS bundle into DIR
  ops                                  list the server ops and notification types the client handles`);
}

function args(argv) {
  const out = { _: [] };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a.startsWith('--')) {
      const key = a.slice(2);
      const next = argv[i + 1];
      if (next === undefined || next.startsWith('--')) out[key] = true;
      else { out[key] = next; i++; }
    } else out._.push(a);
  }
  return out;
}

function redact(value) {
  if (Array.isArray(value)) return value.map(redact);
  if (value && typeof value === 'object') {
    const out = {};
    for (const [k, v] of Object.entries(value)) out[k] = SECRET_KEY.test(k) && typeof v !== 'object' ? '<redacted>' : redact(v);
    return out;
  }
  if (typeof value === 'string') return value.replace(/([?&](?:token|auth|key|signature|idToken)=)[^&\s"]+/gi, '$1<redacted>');
  return value;
}

async function targets() {
  const res = await fetch(`${BASE}/json/list`).catch((e) => {
    throw new Error(`cannot reach ${BASE} (${e.cause?.code || e.message}). Is Kumospace running with --remote-debugging-port, and is the tunnel up?`);
  });
  return res.json();
}

async function page() {
  const list = await targets();
  const target = list.find((t) => t.type === 'page' && /kumospace\.com/.test(t.url));
  if (!target) throw new Error('no Kumospace page target; is the Desktop app signed in and showing a space?');
  const host = new URL(BASE).host;
  const url = target.webSocketDebuggerUrl.replace(/^ws:\/\/[^/]+/, `ws://${host}`);
  return { target, session: await connect(url) };
}

function connect(url) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url);
    let id = 0;
    const pending = new Map();
    const listeners = [];
    ws.onmessage = (e) => {
      const m = JSON.parse(e.data);
      if (m.id && pending.has(m.id)) {
        const { ok, fail } = pending.get(m.id);
        pending.delete(m.id);
        m.error ? fail(new Error(`${m.error.message}`)) : ok(m.result);
      } else if (m.method) listeners.forEach((l) => l(m));
    };
    ws.onerror = () => reject(new Error(`cannot open ${url}`));
    ws.onopen = () => resolve({
      call: (method, params = {}) => new Promise((ok, fail) => {
        const i = ++id;
        pending.set(i, { ok, fail });
        ws.send(JSON.stringify({ id: i, method, params }));
      }),
      on: (fn) => listeners.push(fn),
      close: () => ws.close(),
    });
  });
}

async function evaluate(session, expression) {
  const r = await session.call('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
  return r.result.value;
}

async function bundles(session) {
  const scripts = [];
  session.on((m) => {
    if (m.method === 'Debugger.scriptParsed' && /kumospace\.com\/assets\/.*\.js/.test(m.params.url)) scripts.push(m.params);
  });
  await session.call('Debugger.enable');
  await new Promise((r) => setTimeout(r, 1500));
  const seen = new Map();
  for (const s of scripts) {
    if (seen.has(s.url)) continue;
    const r = await session.call('Debugger.getScriptSource', { scriptId: s.scriptId });
    seen.set(s.url, r.scriptSource || '');
  }
  await session.call('Debugger.disable');
  return seen;
}

function switchBody(src, anchor) {
  const at = src.indexOf(anchor);
  if (at < 0) return null;
  const start = src.lastIndexOf('switch(', at);
  let depth = 0;
  for (let k = src.indexOf('{', start); k < src.length; k++) {
    if (src[k] === '{') depth++;
    else if (src[k] === '}' && --depth === 0) return src.slice(start, k + 1);
  }
  return null;
}

function cases(body) {
  return [...body.matchAll(/case`([A-Za-z0-9_]+)`|case [A-Za-z_$][\w$]*\.([A-Za-z0-9_]+):/g)].map((m) => m[1] || m[2]);
}

async function tail(opts) {
  const { session } = await page();
  const seconds = opts.seconds ? Number(opts.seconds) : 0;
  const sockets = new Map();
  const requests = new Map();
  const write = (entry) => {
    const line = JSON.stringify(opts.raw ? entry : redact(entry));
    if (opts.out) appendFileSync(opts.out, line + '\n');
    else console.log(line);
  };
  session.on(async (m) => {
    const p = m.params;
    const t = new Date().toISOString();
    if (m.method === 'Network.webSocketCreated') {
      sockets.set(p.requestId, p.url);
      write({ t, kind: 'socket', url: p.url });
    } else if (m.method === 'Network.webSocketFrameReceived' || m.method === 'Network.webSocketFrameSent') {
      const url = sockets.get(p.requestId);
      const text = p.response.payloadData;
      let frame = null;
      try { frame = JSON.parse(text); } catch {}

      const hasOp = !!(frame && typeof frame === 'object' && typeof frame.op === 'string');
      const office = url ? url.startsWith(OFFICE_SOCKET) : hasOp;
      if (!office && !opts['all-sockets']) return;
      write({ t, kind: 'ws', dir: m.method.endsWith('Sent') ? 'out' : 'in', socket: url || null, op: hasOp ? frame.op : null, data: hasOp ? frame.data ?? null : frame ?? text });
    } else if (opts.http && m.method === 'Network.requestWillBeSent' && /^https?:/.test(p.request.url)) {
      requests.set(p.requestId, { method: p.request.method, url: p.request.url, body: p.request.postData });
    } else if (opts.http && m.method === 'Network.responseReceived' && requests.has(p.requestId)) {
      const req = requests.get(p.requestId);
      if (!/kumospace\.com|sendbird\.com|daily\.co/.test(req.url)) return;
      const entry = { t, kind: 'http', method: req.method, url: req.url, status: p.response.status, mime: p.response.mimeType, request: req.body ?? undefined };
      if (opts.bodies && /json/.test(p.response.mimeType)) {
        await new Promise((r) => setTimeout(r, 50));
        const body = await session.call('Network.getResponseBody', { requestId: p.requestId }).catch(() => null);
        if (body && !body.base64Encoded) { try { entry.response = JSON.parse(body.body); } catch { entry.response = body.body.slice(0, 2000); } }
      }
      write(entry);
    }
  });
  await session.call('Network.enable', { maxPostDataSize: 65536 });
  console.error(`tailing ${seconds ? seconds + ' s' : 'until Ctrl-C'}${opts.out ? ' into ' + opts.out : ''}`);
  if (seconds) {
    await new Promise((r) => setTimeout(r, seconds * 1000));
    await session.call('Network.disable').catch(() => {});
    session.close();
  }
}

async function main() {
  const [command, ...rest] = process.argv.slice(2);
  const opts = args(rest);
  switch (command) {
    case 'targets': {
      for (const t of await targets()) console.log(`${t.type}\t${t.url}`);
      break;
    }
    case 'tail':
      await tail(opts);
      break;
    case 'state': {
      const { session } = await page();
      const path = opts._[0] || '';
      const expr = `(() => { const s = ${FIND_STORE}; if (!s) return { error: "Redux store not found" };
        const st = s.getState(); const path = ${JSON.stringify(path)};
        if (!path) return Object.fromEntries(Object.entries(st).map(([k, v]) => [k, v && typeof v === "object" ? Object.keys(v) : v]));
        return JSON.parse(JSON.stringify(path.split(".").reduce((o, k) => o == null ? o : o[k], st) ?? null)); })()`;
      console.log(JSON.stringify(redact(await evaluate(session, expr)), null, 2));
      session.close();
      break;
    }
    case 'eval': {
      const { session } = await page();
      console.log(JSON.stringify(redact(await evaluate(session, opts._.join(' '))), null, 2));
      session.close();
      break;
    }
    case 'grep': {
      const { session } = await page();
      const context = Number(opts.context || 300);
      const max = Number(opts.max || 8);
      for (const [url, src] of await bundles(session)) {
        for (const term of opts._) {
          let i = -1, n = 0;
          while ((i = src.indexOf(term, i + 1)) >= 0 && n++ < max) {
            console.log(`== ${url.split('/').pop()} @${i} [${term}]\n${src.slice(Math.max(0, i - context), i + term.length + context)}\n`);
          }
        }
      }
      session.close();
      break;
    }
    case 'dump': {
      const dir = opts._[0];
      if (!dir) throw new Error('dump needs a directory');
      mkdirSync(dir, { recursive: true });
      const { session } = await page();
      for (const [url, src] of await bundles(session)) writeFileSync(join(dir, url.split('/').pop()), src);
      console.log(`saved into ${dir}`);
      session.close();
      break;
    }
    case 'ops': {
      const { session } = await page();
      for (const [url, src] of await bundles(session)) {
        const server = switchBody(src, 'case`census`');
        if (!server) continue;
        const notes = switchBody(src, 'case`doorKnock`');
        console.log(`server ops (${url.split('/').pop()}):\n  ${cases(server).join(', ')}`);
        if (notes) console.log(`notification types (${url.split('/').pop()}):\n  ${cases(notes).join(', ')}`);
        const sent = src.match(/e\.chat=`chat`,e\.privateMessage[^}]*\}/);
        if (sent) console.log(`client op enum (${url.split('/').pop()}):\n  ${[...sent[0].matchAll(/e\.(\w+)=`/g)].map((m) => m[1]).join(', ')}`);
      }
      session.close();
      break;
    }
    default:
      usage();
      process.exitCode = command ? 2 : 0;
  }
}

main().catch((e) => {
  console.error(`kumo-cdp: ${e.message}`);
  process.exit(1);
});
