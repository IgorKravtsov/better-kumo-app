// Minimal CDP client: connects to the browser endpoint, auto-attaches to every
// page/worker target (flattened sessions) and lets callers subscribe to events.
const WebSocket = require('ws');

async function getBrowserWsUrl(port = 9222) {
  const res = await fetch(`http://127.0.0.1:${port}/json/version`);
  return (await res.json()).webSocketDebuggerUrl;
}

class CDP {
  constructor(url) {
    this.url = url;
    this.id = 0;
    this.pending = new Map();
    this.handlers = [];
    this.sessions = new Map(); // sessionId -> targetInfo
  }

  connect() {
    return new Promise((resolve, reject) => {
      this.ws = new WebSocket(this.url, { perMessageDeflate: false, maxPayload: 256 * 1024 * 1024 });
      this.ws.on('open', resolve);
      this.ws.on('error', reject);
      this.ws.on('close', () => this.onClose && this.onClose());
      this.ws.on('message', (raw) => this._onMessage(JSON.parse(raw)));
    });
  }

  send(method, params = {}, sessionId) {
    const id = ++this.id;
    const msg = { id, method, params };
    if (sessionId) msg.sessionId = sessionId;
    this.ws.send(JSON.stringify(msg));
    return new Promise((resolve, reject) => this.pending.set(id, { resolve, reject, method }));
  }

  on(fn) { this.handlers.push(fn); }

  _onMessage(msg) {
    if (msg.id && this.pending.has(msg.id)) {
      const p = this.pending.get(msg.id);
      this.pending.delete(msg.id);
      msg.error ? p.reject(new Error(`${p.method}: ${msg.error.message}`)) : p.resolve(msg.result);
      return;
    }
    if (msg.method === 'Target.attachedToTarget') {
      const { sessionId, targetInfo } = msg.params;
      // Auto-attach and explicit attach can both hit the same target: keep one session.
      if ([...this.sessions.values()].some((t) => t.targetId === targetInfo.targetId)) {
        this.send('Target.detachFromTarget', { sessionId }).catch(() => {});
        return;
      }
      this.sessions.set(sessionId, targetInfo);
      this._setupSession(sessionId, targetInfo).catch(() => {});
    } else if (msg.method === 'Target.detachedFromTarget') {
      this.sessions.delete(msg.params.sessionId);
    }
    for (const h of this.handlers) {
      try { h(msg); } catch (e) { console.error('handler error', e); }
    }
  }

  async _setupSession(sessionId, targetInfo) {
    // Child targets (workers, iframes) of this target get attached too.
    await this.send('Target.setAutoAttach', { autoAttach: true, waitForDebuggerOnStart: true, flatten: true }, sessionId).catch(() => {});
    await this.send('Network.enable', { maxPostDataSize: 65536 }, sessionId).catch(() => {});
    if (targetInfo.type === 'page') await this.send('Runtime.enable', {}, sessionId).catch(() => {});
    await this.send('Runtime.runIfWaitingForDebugger', {}, sessionId).catch(() => {});
    if (this.onSession) this.onSession(sessionId, targetInfo);
  }

  async attachAll() {
    await this.send('Target.setDiscoverTargets', { discover: true });
    await this.send('Target.setAutoAttach', { autoAttach: true, waitForDebuggerOnStart: false, flatten: true });
    const { targetInfos } = await this.send('Target.getTargets');
    for (const t of targetInfos) {
      if (['page', 'service_worker', 'shared_worker', 'worker'].includes(t.type) && !t.attached) {
        await this.send('Target.attachToTarget', { targetId: t.targetId, flatten: true }).catch(() => {});
      }
    }
  }

  // Evaluate JS in the main Kumospace page.
  async evalInPage(expression) {
    for (const [sid, t] of this.sessions) {
      if (t.type === 'page' && /kumospace\.com/.test(t.url)) {
        const r = await this.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true }, sid);
        return r.result.value;
      }
    }
    throw new Error('Kumospace page not attached');
  }
}

module.exports = { CDP, getBrowserWsUrl };
