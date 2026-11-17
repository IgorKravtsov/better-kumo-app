// Replays a frames.jsonl dump through the parser + rules with ntfy stubbed.
process.chdir(__dirname);
const fs = require('fs');
const sent = [];
global.fetch = async (url, opts) => { sent.push(JSON.parse(opts.body)); return { ok: true }; };
const kw = require('./kumowatch');
kw.state.availability = null; kw.state.me = null;
require('./rules')({ on: kw.on, state: kw.state, log: () => {} });
for (const line of fs.readFileSync(process.argv[2] || 'frames.jsonl', 'utf8').trim().split('\n')) {
  const { data } = JSON.parse(line);
  if (/^[A-Z]{4}\{/.test(data)) kw.handleSendbirdFrame(data); else kw.handleKumoFrame(data);
}
setTimeout(() => { console.log('ntfy calls:', JSON.stringify(sent.map(s => ({ title: s.title, msg: s.message.slice(0, 40) })), null, 1)); }, 100);
