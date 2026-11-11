// kumowatch: attaches to the Kumospace Electron app over CDP, turns its
// WebSocket traffic into high-level events and runs rules from rules.js.
//
// Kumospace must be started with --remote-debugging-port=9222 (see start-kumospace.sh).
const fs = require('fs');
const path = require('path');
const { CDP, getBrowserWsUrl } = require('./cdp');

const PORT = Number(process.env.KUMO_CDP_PORT || 9222);
const NEAR_RADIUS = Number(process.env.KUMO_NEAR_RADIUS || 300); // px on the open floor
const DEBUG_LOG = process.env.KUMO_DEBUG_LOG; // optional path: dump raw frames

const STATE_FILE = path.join(__dirname, '.kumowatch-state.json');
const saved = (() => { try { return JSON.parse(fs.readFileSync(STATE_FILE, 'utf8')); } catch { return {}; } })();
const state = {
  me: saved.me ?? null, // { id, firebaseUid, name }
  availability: saved.availability ?? null, // restored until Kumospace sends a fresh one; 'available' | 'away' | 'focusing' | ... (as sent by Kumospace)
  statusMessage: '',
  space: null,
  people: new Map(), // entity id -> { name, availability, x, y, zoneId, roomName }
  pos: null,         // my { x, y, zoneId, roomName }
  near: new Set(),   // ids of people currently near me
};

const listeners = new Map();
const emit = (type, payload) => {
  for (const fn of listeners.get(type) || []) Promise.resolve().then(() => fn(payload, state)).catch((e) => console.error(`[rule ${type}]`, e));
  for (const fn of listeners.get('*') || []) Promise.resolve().then(() => fn({ type, ...payload }, state)).catch(() => {});
};
const on = (type, fn) => { if (!listeners.has(type)) listeners.set(type, []); listeners.get(type).push(fn); };

const log = (...a) => console.log(new Date().toISOString().slice(11, 19), ...a);
const debug = DEBUG_LOG ? fs.createWriteStream(DEBUG_LOG, { flags: 'a' }) : null;

// ---------- Kumospace protocol (ws.kumospace.com) ----------
function handleKumoFrame(raw) {
  let msg;
  try { msg = JSON.parse(raw); } catch { return; }
  const { op, data } = msg;
  if (op === 'config' && data) state.space = { name: data.name, uid: data.uid, displayName: data.displayName };
  if (op === 'census' && data) for (const u of Object.values(data)) if (u) updatePerson(u, u.isSelf, false);
  if (op === 'entities' && Array.isArray(data)) for (const u of data) if (u) updatePerson(u, u.self, true);
  if (op === 'notification' && data) emit('kumo.notification', data);
  emit('kumo.raw', msg);
}

function updatePerson(u, isSelf, hasPosition) {
  if (!u || u.type !== 'user' || !u.userStatus) return;
  const availability = u.userStatus.availability;
  const prev = state.people.get(u.id);
  const person = { ...(prev || {}), name: u.name, availability };
  // "entities" frames carry the position; a missing zoneId there means "open floor".
  if (hasPosition) Object.assign(person, { x: u.x, y: u.y, zoneId: u.zoneId || null, roomName: u.roomName });
  state.people.set(u.id, person);
  if (hasPosition) updateProximity(u.id, person, isSelf);
  if (isSelf) {
    if (!state.me || !state.me.firebaseUid) state.me = { id: u.id, name: u.name, firebaseUid: u.firebaseUid || state.me?.firebaseUid };
    if (availability !== state.availability || (u.userStatus.message || '') !== state.statusMessage) {
      const from = state.availability;
      state.availability = availability;
      state.statusMessage = u.userStatus.message || '';
      log(`my status: ${from} -> ${availability}${state.statusMessage ? ` (${state.statusMessage})` : ''}`);
      try { fs.writeFileSync(STATE_FILE, JSON.stringify({ availability, me: state.me })); } catch {}
      emit('status.self', { from, to: availability, message: state.statusMessage });
    }
  } else if (prev && prev.availability !== availability) {
    emit('status.other', { id: u.id, name: u.name, from: prev.availability, to: availability });
  }
}

// "Near" = same zone (office/room) as me, or within NEAR_RADIUS when we're both on the open floor.
function isNear(p, me) {
  if (!me || p.roomName !== me.roomName || p.x == null) return false;
  if (me.zoneId || p.zoneId) return me.zoneId === p.zoneId;
  return Math.hypot(p.x - me.x, p.y - me.y) <= NEAR_RADIUS;
}

function updateProximity(id, person, isSelf) {
  if (isSelf) {
    // I moved: just recompute who is near, never notify for people I walked up to.
    state.pos = person;
    state.near = new Set([...state.people].filter(([pid, p]) => pid !== id && isNear(p, person)).map(([pid]) => pid));
    return;
  }
  const near = isNear(person, state.pos);
  if (near && !state.near.has(id)) {
    state.near.add(id);
    const distance = Math.round(Math.hypot(person.x - state.pos.x, person.y - state.pos.y));
    log(`approached: ${person.name} (${person.zoneId ? 'same zone' : distance + 'px'})`);
    emit('person.approached', { id, name: person.name, sameZone: !!person.zoneId, distance, myStatus: state.availability });
  } else if (!near && state.near.has(id)) {
    state.near.delete(id);
    emit('person.left', { id, name: person.name, myStatus: state.availability });
  }
}

// ---------- Sendbird protocol (chat) ----------
const seenMessages = new Set();
function handleSendbirdFrame(raw) {
  const cmd = raw.slice(0, 4);
  let body;
  try { body = JSON.parse(raw.slice(4)); } catch { return; }
  if (cmd === 'LOGI' && body.user_id) {
    state.me = { ...(state.me || {}), firebaseUid: body.user_id, name: state.me?.name || body.nickname };
    return;
  }
  if (cmd !== 'MESG' && cmd !== 'FILE' && cmd !== 'MEDI') return;
  const id = body.msg_id;
  if (id) { if (seenMessages.has(id)) return; seenMessages.add(id); if (seenMessages.size > 5000) seenMessages.clear(); }
  const sender = body.user || {};
  const myUid = state.me?.firebaseUid;
  const channelUrl = body.channel_url || '';
  const firstName = (state.me?.name || '').split(' ')[0];
  const mentioned = (body.mentioned_users || []).some((m) => (m.user_id || m.guest_id) === myUid)
    || body.mention_type === 'channel'
    || (!!firstName && new RegExp(`@${firstName}\\b`, 'i').test(body.message || ''));
  const evt = {
    id,
    channelUrl,
    channelType: classifyChannel(channelUrl, body),
    channelName: body.channel?.name || '',
    text: cmd === 'MESG' ? (body.message || '') : (body.message || `[file] ${body.name || body.file?.name || ''}`),
    sender: { id: sender.guest_id || sender.user_id, name: sender.name || sender.nickname || 'Someone' },
    fromMe: !!myUid && (sender.guest_id || sender.user_id) === myUid,
    mentionsMe: mentioned,
    myStatus: state.availability, // my availability at the moment the message arrived
    customType: body.custom_type,
    raw: body,
  };
  log(`chat [${evt.channelType}] ${evt.sender.name}: ${evt.text.slice(0, 80)}`);
  emit('chat.message', evt);
}

function classifyChannel(url, body) {
  if (/_group_dms_/.test(url)) return 'group_dm';
  // 1:1 DMs look like "<spaceUid>_<firebaseUidA>_<firebaseUidB>"
  const parts = url.split('_');
  const myUid = state.me?.firebaseUid;
  if (parts.length === 3 && myUid && parts.slice(1).includes(myUid)) return 'dm';
  if (body.channel?.is_distinct) return 'dm';
  if (/nearby/i.test(url)) return 'nearby';
  if (/room/i.test(url)) return 'room';
  return 'channel';
}

// ---------- CDP wiring with auto-reconnect ----------
async function run() {
  const cdp = new CDP(await getBrowserWsUrl(PORT));
  await cdp.connect();
  const sockets = new Map();
  cdp.on((m) => {
    const p = m.params || {};
    if (m.method === 'Network.webSocketCreated') { sockets.set(p.requestId, p.url); log('ws open', p.url.split('?')[0]); return; }
    if (m.method !== 'Network.webSocketFrameReceived') return;
    const url = sockets.get(p.requestId) || p.url || '';
    const data = p.response.payloadData;
    if (p.response.opcode !== 1) return; // text frames only
    if (debug) debug.write(JSON.stringify({ t: Date.now(), url: url.split('?')[0], data }) + '\n');
    if (/sendbird\.com/.test(url) || /^[A-Z]{4}\{/.test(data)) handleSendbirdFrame(data);
    else handleKumoFrame(data);
  });
  // New windows/reloads: attach to newly created targets as well.
  cdp.on((m) => {
    if (m.method === 'Target.targetCreated' && m.params.targetInfo.type === 'page' && !m.params.targetInfo.attached) {
      cdp.send('Target.attachToTarget', { targetId: m.params.targetInfo.targetId, flatten: true }).catch(() => {});
    }
  });
  await cdp.attachAll();
  state.cdp = cdp;
  log('attached to Kumospace');
  emit('connected', {});
  await new Promise((resolve) => { cdp.onClose = resolve; });
  state.cdp = null;
  log('Kumospace connection closed');
  emit('disconnected', {});
}

async function main() {
  if (state.availability) log('restored status:', state.availability);
  const rules = require(path.resolve(__dirname, process.env.KUMO_RULES || './rules.js'));
  rules({ on, state, log });
  if (process.env.KUMO_WATCHDOG !== '0') require('./launcher').startWatchdog({ port: PORT, log, on });
  for (;;) {
    try { await run(); } catch (e) { log('waiting for Kumospace on port', PORT, '-', e.message); }
    await new Promise((r) => setTimeout(r, 5000));
  }
}

module.exports = { handleKumoFrame, handleSendbirdFrame, state, on };
if (require.main === module) main();
