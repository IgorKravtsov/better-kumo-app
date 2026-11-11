// Rules: add your own reactions to Kumospace events here.
//
// Events:
//   chat.message      { channelType: dm|group_dm|channel|room|nearby, sender, text, mentionsMe, fromMe, myStatus, ... }
//   person.approached { id, name, sameZone, distance, myStatus } — someone walked up to you / into your office
//   person.left       { id, name, myStatus }
//   status.self       { from, to, message }          — your availability changed
//   status.other      { id, name, from, to }         — someone else's availability changed
//   kumo.notification { message, type }              — toasts like "X joined the Floor"
//   kumo.raw          raw ws.kumospace.com frame     — everything else
//   connected / disconnected
const NTFY_SERVER = process.env.NTFY_SERVER || 'https://ntfy.sh';
const NTFY_TOPIC = process.env.NTFY_TOPIC || 'kumospace_ihor';

// Your availabilities in which things are forwarded to the phone.
const FORWARD_WHEN = new Set((process.env.FORWARD_WHEN || 'away,focusing').split(','));
// Chats forwarded in full. In other channels only messages that mention you.
const FORWARD_CHANNELS = new Set(['dm', 'group_dm']);
// Ignore re-entries within this window (someone jittering on a zone border); every real visit notifies.
const APPROACH_COOLDOWN_MS = 60 * 1000;

async function ntfy({ title, message, tags = [], priority = 3, click }) {
  const res = await fetch(NTFY_SERVER, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ topic: NTFY_TOPIC, title, message, tags, priority, click }),
  });
  if (!res.ok) throw new Error(`ntfy ${res.status}: ${await res.text()}`);
}

module.exports = ({ on, state, log }) => {
  const spaceUrl = () => (state.space ? `https://www.kumospace.com/${state.space.name}` : undefined);

  // 1. Messages (DMs, group DMs, mentions in channels) while away/focusing.
  on('chat.message', async (m) => {
    if (m.fromMe) return;
    if (!FORWARD_WHEN.has(m.myStatus)) return;
    if (!FORWARD_CHANNELS.has(m.channelType) && !m.mentionsMe) return;
    const where = m.channelType === 'dm' ? '' : ` in ${m.channelName || m.channelType}`;
    await ntfy({
      title: `${m.mentionsMe && m.channelType !== 'dm' ? '@ ' : ''}${m.sender.name}${where}`,
      message: m.text || '(no text)',
      tags: [m.mentionsMe ? 'mega' : 'speech_balloon'],
      priority: m.channelType === 'dm' || m.mentionsMe ? 4 : 3,
      click: spaceUrl(),
    });
    log(`→ ntfy: message from ${m.sender.name}`);
  });

  // 2. Someone walked up to you while you're away/focusing.
  const lastApproach = new Map();
  on('person.approached', async (p) => {
    if (!FORWARD_WHEN.has(p.myStatus)) return;
    if (Date.now() - (lastApproach.get(p.id) || 0) < APPROACH_COOLDOWN_MS) { log(`(skip: ${p.name} re-entered within cooldown)`); return; }
    lastApproach.set(p.id, Date.now());
    await ntfy({
      title: `${p.name} is at your desk`,
      message: p.sameZone ? `${p.name} came into your office in Kumospace` : `${p.name} walked up to you in Kumospace`,
      tags: ['wave'],
      priority: 4,
      click: spaceUrl(),
    });
    log(`→ ntfy: ${p.name} approached`);
  });

  // Example: push whenever your own status changes.
  // on('status.self', ({ from, to }) => ntfy({ title: 'Kumospace status', message: `${from} → ${to}` }));
};
